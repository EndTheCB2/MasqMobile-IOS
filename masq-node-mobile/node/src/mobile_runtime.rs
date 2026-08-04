// Copyright (c) 2026, MASQ (https://masq.ai) and/or its affiliates.
//
// Small process-local control surface for embedding the existing Node actor system in a mobile
// application. It deliberately exposes no serving controls: the mobile launcher is accepted only
// together with `--neighborhood-mode consume-only` by the configurator.

use crate::sub_lib::neighborhood::{
    ConfigChange, ConfigChangeMsg, Hops, RenewRouteReadinessLeaseMessage,
};
use crate::sub_lib::peer_actors::StartMessage;
use actix::msgs::StopArbiter;
use actix::{Addr, Arbiter, Recipient, System};
use masq_lib::messages::{CountryGroups, ToMessageBody, UiSetExitLocationRequest};
use masq_lib::ui_gateway::{MessageBody, NodeFromUiMessage};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::str::FromStr;
use std::sync::atomic::{
    AtomicBool, AtomicI32, AtomicU16, AtomicU64, AtomicU8, AtomicUsize, Ordering,
};
use std::sync::{mpsc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const NO_EXIT_CODE: i32 = i32::MIN;
const ROUTE_READINESS_RENEWAL_ACK_TIMEOUT: Duration = Duration::from_millis(750);

static EMBEDDED: AtomicBool = AtomicBool::new(false);
static STARTED: AtomicBool = AtomicBool::new(false);
static STOP_REQUESTED: AtomicBool = AtomicBool::new(false);
static PROXY_PORT: AtomicU16 = AtomicU16::new(0);
static ROUTE_STAGE: AtomicU8 = AtomicU8::new(0);
static ROUTE_HOPS: AtomicU8 = AtomicU8::new(0);
static ROUTE_PROOF_GENERATION: AtomicU64 = AtomicU64::new(0);
static RUNTIME_EPOCH: AtomicU64 = AtomicU64::new(0);
static RUNTIME_LIFECYCLE: Mutex<()> = Mutex::new(());
static BYTES_UP: AtomicU64 = AtomicU64::new(0);
static BYTES_DOWN: AtomicU64 = AtomicU64::new(0);
static ENTRY_HANDSHAKE_PROGRESS: Mutex<EntryHandshakeProgress> =
    Mutex::new(EntryHandshakeProgress {
        milestone: EntryHandshakeMilestone::None,
        attempt_started_at: None,
        last_activity_at: None,
    });
static LAST_EXIT_CODE: AtomicI32 = AtomicI32::new(NO_EXIT_CODE);
static SYSTEM: Mutex<Option<System>> = Mutex::new(None);
static ACTOR_ARBITERS: Mutex<Vec<Addr<Arbiter>>> = Mutex::new(Vec::new());
static EXPECTED_ACTOR_ARBITERS: AtomicUsize = AtomicUsize::new(0);
static REGISTERED_ACTOR_ARBITERS: AtomicUsize = AtomicUsize::new(0);
static ACTOR_ARBITER_TRACKING_ACTIVE: AtomicBool = AtomicBool::new(false);
static ACTOR_ARBITER_SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);
static LAST_CONNECTION_ERROR: Mutex<Option<String>> = Mutex::new(None);
const CORE_PANIC_LOCATION_PREFIX: &str = "E_CORE_PANIC_LOCATION:";
static AVAILABLE_EXIT_COUNTRIES: Mutex<Vec<String>> = Mutex::new(Vec::new());
static EXIT_PREFERENCE: Mutex<Option<MobileExitPreference>> = Mutex::new(None);
static NEIGHBORHOOD_UI: Mutex<Option<Recipient<NodeFromUiMessage>>> = Mutex::new(None);
static MIN_HOPS_PREFERENCE: AtomicU8 = AtomicU8::new(0);
static NEIGHBORHOOD_CONFIG: Mutex<Option<Recipient<ConfigChangeMsg>>> = Mutex::new(None);
static NEIGHBORHOOD_RETRY: Mutex<Option<Recipient<StartMessage>>> = Mutex::new(None);
static NEIGHBORHOOD_ROUTE_READINESS_RENEWAL: Mutex<Option<RouteReadinessRenewalTarget>> =
    Mutex::new(None);
static PENDING_ENTRY_NODES: Mutex<Option<Vec<String>>> = Mutex::new(None);
static STREAM_CONNECT_JOBS: Mutex<usize> = Mutex::new(0);
static STREAM_CONNECT_JOBS_FINISHED: Condvar = Condvar::new();

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MobileConnectionError {
    StreamConnectionFailed,
    PassLoopFound,
    NoEntryProgress,
    RouteLengthPreferenceFailed,
    ExitCountryPreferenceFailed,
}

impl MobileConnectionError {
    fn message(self) -> &'static str {
        match self {
            Self::StreamConnectionFailed => "Stream connection failed",
            Self::PassLoopFound => {
                "E_ENTRY_GOSSIP_PASS_LOOP: The entry-node handshake encountered a pass loop"
            }
            Self::NoEntryProgress => {
                "E_ENTRY_NO_PROGRESS: All selected entry peers exhausted the initial handshake"
            }
            Self::RouteLengthPreferenceFailed => "Route length could not be applied",
            Self::ExitCountryPreferenceFailed => "Exit-country preference could not be applied",
        }
    }

    fn takes_priority_over(self, current: Option<&str>) -> bool {
        !current
            .map(|message| message.starts_with(CORE_PANIC_LOCATION_PREFIX))
            .unwrap_or(false)
            && match self {
                // Neighborhood emits NoEntryProgress only after every
                // parallel entry attempt is terminal. That aggregate signal
                // is stronger than a pass loop from one attempt and must
                // remain stable when late per-peer events arrive.
                Self::NoEntryProgress => true,
                Self::PassLoopFound => !current
                    .map(|message| message.starts_with("E_ENTRY_NO_PROGRESS:"))
                    .unwrap_or(false),
                Self::StreamConnectionFailed => !current
                    .map(|message| {
                        message.starts_with("E_ENTRY_GOSSIP_PASS_LOOP:")
                            || message.starts_with("E_ENTRY_NO_PROGRESS:")
                    })
                    .unwrap_or(false),
                Self::RouteLengthPreferenceFailed | Self::ExitCountryPreferenceFailed => !current
                    .map(|message| {
                        message.starts_with("E_ENTRY_GOSSIP_PASS_LOOP:")
                            || message.starts_with("E_ENTRY_NO_PROGRESS:")
                    })
                    .unwrap_or(false),
            }
    }
}

/// Privacy-safe, process-local progress through the current entry-node handshake.
///
/// The value is deliberately absent from `MobileRuntimeSnapshot` and therefore
/// from the app's public status model. It records only a monotone transport
/// phase: never an address, identity, descriptor, payload, or byte count.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(u8)]
pub enum EntryHandshakeMilestone {
    None = 0,
    TcpConnected = 1,
    DebutBytesWritten = 2,
    InboundBytesReceived = 3,
    GossipAccepted = 4,
}

#[derive(Debug)]
struct EntryHandshakeProgress {
    milestone: EntryHandshakeMilestone,
    attempt_started_at: Option<Instant>,
    last_activity_at: Option<Instant>,
}

/// Keeps an embedded connector job visible until its blocking socket attempt
/// has actually returned, even if the future waiting for it is dropped.
pub struct StreamConnectJobGuard {
    tracked: bool,
}

impl Drop for StreamConnectJobGuard {
    fn drop(&mut self) {
        if !self.tracked {
            return;
        }
        let mut jobs = STREAM_CONNECT_JOBS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *jobs = jobs
            .checked_sub(1)
            .expect("stream-connect job tracking underflow");
        if *jobs == 0 {
            STREAM_CONNECT_JOBS_FINISHED.notify_all();
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct MobileExitPreference {
    country_code: Option<String>,
    fallback_routing: bool,
}

#[derive(Clone)]
struct RouteReadinessRenewalTarget {
    mobile_runtime_epoch: u64,
    recipient: Recipient<RenewRouteReadinessLeaseMessage>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MobileRuntimeSnapshot {
    pub started: bool,
    pub stop_requested: bool,
    pub proxy_port: Option<u16>,
    /// 0 = not connected, 1 = connected to a neighbor, 2 = usable route found.
    pub route_stage: u8,
    /// Actual outward MASQ hops in the most recently selected route.
    pub route_hops: u8,
    /// Monotone proof-of-use counter. It contains no route, peer, or traffic identity.
    pub route_proof_generation: u64,
    pub bytes_up: u64,
    pub bytes_down: u64,
    pub last_exit_code: Option<i32>,
    pub last_connection_error: Option<String>,
    pub available_exit_countries: Vec<String>,
}

/// Runs an embedded Node after the caller has prepared the runtime and applied
/// its preferences. Keeping preparation outside this function prevents actor
/// startup from clearing preferences that were queued just before spawning.
pub fn run_embedded(args: &[String]) -> i32 {
    install_early_panic_location_hook();
    let exit_code = catch_unwind(AssertUnwindSafe(|| {
        crate::sub_lib::main_tools::main_with_args(args)
    }))
    .unwrap_or(101);
    finish(exit_code);
    exit_code
}

fn install_early_panic_location_hook() {
    std::panic::set_hook(Box::new(|panic_info| {
        if let Some(location) = panic_info.location() {
            report_panic_location(location.file(), location.line());
        } else {
            report_panic_location("unknown", 0);
        }
    }));
}

pub fn prepare(proxy_port: u16) {
    let _lifecycle = RUNTIME_LIFECYCLE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    // A previous embedded actor system may have exited in this same app process. Its UI Gateway
    // recipient is no longer valid and must not receive startup logs from the next system.
    masq_lib::logger::clear_log_recipient();
    advance_runtime_epoch();
    EMBEDDED.store(true, Ordering::SeqCst);
    STARTED.store(false, Ordering::SeqCst);
    STOP_REQUESTED.store(false, Ordering::SeqCst);
    PROXY_PORT.store(proxy_port, Ordering::SeqCst);
    ROUTE_STAGE.store(0, Ordering::SeqCst);
    ROUTE_HOPS.store(0, Ordering::SeqCst);
    ROUTE_PROOF_GENERATION.store(0, Ordering::SeqCst);
    BYTES_UP.store(0, Ordering::SeqCst);
    BYTES_DOWN.store(0, Ordering::SeqCst);
    reset_entry_handshake_milestone();
    LAST_EXIT_CODE.store(NO_EXIT_CODE, Ordering::SeqCst);
    *LAST_CONNECTION_ERROR
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    AVAILABLE_EXIT_COUNTRIES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
    *SYSTEM
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    ACTOR_ARBITERS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
    EXPECTED_ACTOR_ARBITERS.store(0, Ordering::SeqCst);
    REGISTERED_ACTOR_ARBITERS.store(0, Ordering::SeqCst);
    ACTOR_ARBITER_TRACKING_ACTIVE.store(true, Ordering::SeqCst);
    ACTOR_ARBITER_SHUTDOWN_REQUESTED.store(false, Ordering::SeqCst);
    *NEIGHBORHOOD_UI
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    MIN_HOPS_PREFERENCE.store(0, Ordering::SeqCst);
    *NEIGHBORHOOD_CONFIG
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    *NEIGHBORHOOD_RETRY
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    *NEIGHBORHOOD_ROUTE_READINESS_RENEWAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    *PENDING_ENTRY_NODES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
}

pub fn is_embedded() -> bool {
    EMBEDDED.load(Ordering::SeqCst)
}

fn advance_runtime_epoch() -> u64 {
    RUNTIME_EPOCH
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |current| {
            current.checked_add(1)
        })
        .map(|previous| previous + 1)
        .expect("the mobile runtime epoch is exhausted")
}

fn prepared_runtime_epoch_unlocked() -> Option<u64> {
    let epoch = RUNTIME_EPOCH.load(Ordering::SeqCst);
    (is_embedded() && !STOP_REQUESTED.load(Ordering::SeqCst) && epoch > 0).then_some(epoch)
}

/// Confirms only the anonymous lifecycle generation of the currently running embedded Node.
pub(crate) fn runtime_epoch_is_current(epoch: u64) -> bool {
    let _lifecycle = RUNTIME_LIFECYCLE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    runtime_epoch_is_current_unlocked(epoch)
}

fn runtime_epoch_is_current_unlocked(epoch: u64) -> bool {
    epoch > 0
        && prepared_runtime_epoch_unlocked() == Some(epoch)
        && STARTED.load(Ordering::SeqCst)
        && !ACTOR_ARBITER_SHUTDOWN_REQUESTED.load(Ordering::SeqCst)
}

#[cfg(test)]
pub(crate) fn current_runtime_epoch_for_test() -> Option<u64> {
    let epoch = RUNTIME_EPOCH.load(Ordering::SeqCst);
    runtime_epoch_is_current(epoch).then_some(epoch)
}

pub fn is_stop_requested() -> bool {
    (is_embedded() && STOP_REQUESTED.load(Ordering::SeqCst))
        || ACTOR_ARBITER_SHUTDOWN_REQUESTED.load(Ordering::SeqCst)
}

pub fn register_system(system: System) {
    if is_embedded() {
        *SYSTEM
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(system.clone());
        if STOP_REQUESTED.load(Ordering::SeqCst) {
            system.stop();
        }
    }
}

pub fn expect_actor_arbiter() {
    let _arbiters = ACTOR_ARBITERS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if ACTOR_ARBITER_TRACKING_ACTIVE.load(Ordering::SeqCst) {
        EXPECTED_ACTOR_ARBITERS.fetch_add(1, Ordering::SeqCst);
    }
}

pub fn register_actor_arbiter(arbiter: Addr<Arbiter>) {
    let mut arbiters = ACTOR_ARBITERS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if ACTOR_ARBITER_TRACKING_ACTIVE.load(Ordering::SeqCst) {
        if !arbiters.contains(&arbiter) {
            arbiters.push(arbiter.clone());
            REGISTERED_ACTOR_ARBITERS.fetch_add(1, Ordering::SeqCst);
        }
        if ACTOR_ARBITER_SHUTDOWN_REQUESTED.load(Ordering::SeqCst) {
            arbiter.do_send(StopArbiter(0));
        }
    }
}

pub fn tracked_actor_arbiter_count() -> usize {
    REGISTERED_ACTOR_ARBITERS.load(Ordering::SeqCst)
}

pub fn expected_actor_arbiter_count() -> usize {
    EXPECTED_ACTOR_ARBITERS.load(Ordering::SeqCst)
}

/// Waits until every actor Arbiter created for the embedded Node has dropped its mailbox.
///
/// Actix 0.7 signals the main runner before its auxiliary Arbiter threads necessarily exit.
/// Tracking their addresses turns mobile shutdown into an actual acknowledgement rather than a
/// fixed sleep.
pub fn wait_for_actor_arbiters(timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        let shutdown_confirmed = {
            let mut arbiters = ACTOR_ARBITERS
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            arbiters.retain(Addr::connected);
            let every_expected_arbiter_registered = REGISTERED_ACTOR_ARBITERS
                .load(Ordering::SeqCst)
                >= EXPECTED_ACTOR_ARBITERS.load(Ordering::SeqCst);
            if every_expected_arbiter_registered && arbiters.is_empty() {
                ACTOR_ARBITER_TRACKING_ACTIVE.store(false, Ordering::SeqCst);
                ACTOR_ARBITER_SHUTDOWN_REQUESTED.store(false, Ordering::SeqCst);
                true
            } else {
                false
            }
        };
        if shutdown_confirmed {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(10));
    }
}

pub fn mark_started() {
    let _lifecycle = RUNTIME_LIFECYCLE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if is_embedded() {
        STARTED.store(true, Ordering::SeqCst);
    }
}

pub fn report_route_stage(stage: u8) {
    if is_embedded() {
        let stage = stage.min(2);
        let previous_stage = ROUTE_STAGE.swap(stage, Ordering::SeqCst);
        if stage == 0 {
            ROUTE_HOPS.store(0, Ordering::SeqCst);
            if previous_stage >= 1 {
                reset_entry_handshake_milestone();
            }
        }
        if stage >= 1 {
            clear_terminal_entry_handshake_error();
            apply_exit_preference();
        }
    }
}

/// Records that correlated response data completed an end-to-end MASQ route.
/// The counter lets mobile lifecycle code refresh an idle route before its
/// readiness lease expires, without exposing peer or destination information.
pub fn report_route_use_succeeded() {
    if is_embedded() {
        advance_route_proof_generation(&ROUTE_PROOF_GENERATION);
    }
}

/// Advances proof only while `prepare`/`finish` cannot replace the validated runtime epoch.
///
/// This is reserved for acknowledged synthetic renewal. Genuine correlated route-use telemetry
/// continues to use [report_route_use_succeeded] and keeps its existing semantics.
pub(crate) fn report_route_use_succeeded_for_epoch(mobile_runtime_epoch: u64) -> bool {
    let _lifecycle = RUNTIME_LIFECYCLE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if !runtime_epoch_is_current_unlocked(mobile_runtime_epoch) {
        return false;
    }
    advance_route_proof_generation(&ROUTE_PROOF_GENERATION);
    true
}

fn advance_route_proof_generation(counter: &AtomicU64) {
    let _ = counter.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |current| {
        Some(current.saturating_add(1).max(1))
    });
}

pub fn report_entry_handshake_milestone(milestone: EntryHandshakeMilestone) {
    if !is_embedded() {
        return;
    }
    let mut progress = ENTRY_HANDSHAKE_PROGRESS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if milestone > progress.milestone {
        progress.milestone = milestone;
    }
    // Repeated positive reads/writes are real aggregate transport activity.
    // Refreshing this monotonic clock prevents a slow partial frame from being
    // mistaken for a silent peer without recording a byte count or identity.
    progress.last_activity_at = Some(Instant::now());
}

pub fn entry_handshake_milestone() -> EntryHandshakeMilestone {
    ENTRY_HANDSHAKE_PROGRESS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .milestone
}

/// Returns only the current aggregate milestone, monotonic inactivity, and
/// age of this aggregate attempt. None of these values contains a peer
/// identity, address, descriptor, payload, or byte count.
pub fn entry_handshake_progress() -> (EntryHandshakeMilestone, Duration, Duration) {
    let progress = ENTRY_HANDSHAKE_PROGRESS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    (
        progress.milestone,
        progress
            .last_activity_at
            .map(|last_activity_at| last_activity_at.elapsed())
            .unwrap_or_default(),
        progress
            .attempt_started_at
            .map(|attempt_started_at| attempt_started_at.elapsed())
            .unwrap_or_default(),
    )
}

fn reset_entry_handshake_milestone() {
    let mut progress = ENTRY_HANDSHAKE_PROGRESS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let now = Instant::now();
    progress.milestone = EntryHandshakeMilestone::None;
    progress.attempt_started_at = Some(now);
    progress.last_activity_at = Some(now);
}

fn clear_terminal_entry_handshake_error() {
    let mut last_error = LAST_CONNECTION_ERROR
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if last_error
        .as_deref()
        .map(|message| {
            message.starts_with("E_ENTRY_GOSSIP_PASS_LOOP:")
                || message.starts_with("E_ENTRY_NO_PROGRESS:")
        })
        .unwrap_or(false)
    {
        *last_error = None;
    }
}

pub fn report_route_hops(hops: usize) {
    if is_embedded() {
        ROUTE_HOPS.store(hops.min(u8::MAX as usize) as u8, Ordering::SeqCst);
    }
}

pub fn set_exit_preference(country_code: Option<&str>, fallback_routing: bool) {
    *EXIT_PREFERENCE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(MobileExitPreference {
        country_code: country_code.map(str::to_owned),
        fallback_routing,
    });
}

pub fn register_neighborhood_ui(recipient: Recipient<NodeFromUiMessage>) {
    if !is_embedded() {
        return;
    }
    *NEIGHBORHOOD_UI
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(recipient);
    apply_exit_preference();
}

pub fn register_neighborhood_config(recipient: Recipient<ConfigChangeMsg>) {
    if !is_embedded() {
        return;
    }
    *NEIGHBORHOOD_CONFIG
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(recipient);
    if let Err(_error) = apply_min_hops_preference() {
        report_connection_error(MobileConnectionError::RouteLengthPreferenceFailed);
    }
}

pub fn register_neighborhood_retry(recipient: Recipient<StartMessage>) {
    if !is_embedded() {
        return;
    }
    *NEIGHBORHOOD_RETRY
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(recipient);
}

/// Registers the current embedded Neighborhood actor's acknowledged readiness-renewal mailbox.
///
/// The recipient is process-local and carries no route, peer, destination, stream, or wallet
/// identity. [prepare] and [finish] clear it so a later engine can never renew an older actor's
/// route-readiness lease.
pub fn register_neighborhood_route_readiness_renewal(
    recipient: Recipient<RenewRouteReadinessLeaseMessage>,
) {
    let _lifecycle = RUNTIME_LIFECYCLE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(mobile_runtime_epoch) = prepared_runtime_epoch_unlocked() else {
        return;
    };
    *NEIGHBORHOOD_ROUTE_READINESS_RENEWAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(RouteReadinessRenewalTarget {
        mobile_runtime_epoch,
        recipient,
    });
}

/// Requests one bounded, privacy-safe renewal of the live Neighborhood route-readiness lease.
///
/// `true` requires both an affirmative actor acknowledgement and the same current runtime epoch
/// after that acknowledgement. A missing, stopped, full, timed-out, demoted, stale, or otherwise
/// unavailable actor fails closed and leaves periodic refresh to report failure.
pub fn request_route_readiness_lease_renewal() -> bool {
    let target = NEIGHBORHOOD_ROUTE_READINESS_RENEWAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    let Some(target) = target else {
        return false;
    };
    if !runtime_epoch_is_current(target.mobile_runtime_epoch) {
        return false;
    }
    let (acknowledgement, receiver) = mpsc::sync_channel(1);
    if target
        .recipient
        .try_send(RenewRouteReadinessLeaseMessage {
            mobile_runtime_epoch: target.mobile_runtime_epoch,
            acknowledgement,
        })
        .is_err()
    {
        return false;
    }
    matches!(
        receiver.recv_timeout(ROUTE_READINESS_RENEWAL_ACK_TIMEOUT),
        Ok(true)
    ) && runtime_epoch_is_current(target.mobile_runtime_epoch)
}

/// Re-runs the initial-neighbor handshake without recreating Actix's
/// process-global runtime. The Neighborhood actor resets failed handshakes when
/// it receives StartMessage, so this is a real retry rather than a status poll.
pub fn retry_connection(entry_nodes: &[String]) -> Result<(), String> {
    if !is_embedded() || !STARTED.load(Ordering::SeqCst) {
        return Err("The embedded MASQ Node is not ready to retry.".to_string());
    }
    *PENDING_ENTRY_NODES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) =
        (!entry_nodes.is_empty()).then(|| entry_nodes.to_vec());
    ROUTE_STAGE.store(0, Ordering::SeqCst);
    ROUTE_HOPS.store(0, Ordering::SeqCst);
    reset_entry_handshake_milestone();
    *LAST_CONNECTION_ERROR
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    let recipient = NEIGHBORHOOD_RETRY
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
        .ok_or_else(|| "The MASQ neighborhood is not ready to retry.".to_string())?;
    recipient
        .try_send(StartMessage {})
        .map_err(|_| "The MASQ neighborhood could not restart its handshake.".to_string())
}

/// Called only by the Neighborhood actor while processing a StartMessage. Keeping the freshly
/// discovered descriptors in this process-local handoff avoids changing the widely shared
/// StartMessage type and lets an embedded Node rotate entry nodes without recreating Actix.
pub fn take_pending_entry_nodes() -> Option<Vec<String>> {
    if !is_embedded() {
        return None;
    }
    PENDING_ENTRY_NODES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take()
}

/// Changes route length without restarting the embedded actor system. The
/// Neighborhood actor invalidates its previous route proof and rebuilds future
/// routes using the new minimum.
pub fn set_min_hops(min_hops: u8) -> Result<(), String> {
    Hops::from_str(&min_hops.to_string())?;
    MIN_HOPS_PREFERENCE.store(min_hops, Ordering::SeqCst);
    apply_min_hops_preference()?;
    ROUTE_STAGE.fetch_min(1, Ordering::SeqCst);
    ROUTE_HOPS.store(0, Ordering::SeqCst);
    Ok(())
}

fn apply_min_hops_preference() -> Result<(), String> {
    let min_hops = MIN_HOPS_PREFERENCE.load(Ordering::SeqCst);
    if min_hops == 0 {
        return Ok(());
    }
    let recipient = NEIGHBORHOOD_CONFIG
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    let recipient = match recipient {
        Some(recipient) => recipient,
        None => {
            // Actor startup is asynchronous. Keep the preference pending until
            // register_neighborhood_config is called.
            return Ok(());
        }
    };
    let hops = Hops::from_str(&min_hops.to_string())?;
    recipient
        .try_send(ConfigChangeMsg {
            change: ConfigChange::UpdateMinHops(hops),
        })
        .map_err(|_| "Route length could not be applied".to_string())
}

fn apply_exit_preference() {
    let preference = EXIT_PREFERENCE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    let recipient = NEIGHBORHOOD_UI
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    let (preference, recipient) = match (preference, recipient) {
        (Some(preference), Some(recipient)) => (preference, recipient),
        _ => return,
    };
    let exit_locations = preference
        .country_code
        .map(|country_code| {
            vec![CountryGroups {
                country_codes: vec![country_code],
                priority: 1,
            }]
        })
        .unwrap_or_default();
    let body: MessageBody = UiSetExitLocationRequest {
        fallback_routing: preference.fallback_routing,
        exit_locations,
        show_countries: false,
    }
    .tmb(0);
    if let Err(_error) = recipient.try_send(NodeFromUiMessage { client_id: 0, body }) {
        report_connection_error(MobileConnectionError::ExitCountryPreferenceFailed);
    }
}

pub fn report_connection_error(error: MobileConnectionError) {
    if is_embedded() {
        let mut last_error = LAST_CONNECTION_ERROR
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if error.takes_priority_over(last_error.as_deref()) {
            *last_error = Some(error.message().to_string());
        }
    }
}

pub fn report_panic_location(file: &str, line: u32) {
    if !is_embedded() {
        return;
    }
    let file_name = Path::new(file)
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| {
            name.chars()
                .filter(|character| {
                    character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.')
                })
                .take(64)
                .collect::<String>()
        })
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "unknown".to_owned());
    *LAST_CONNECTION_ERROR
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) =
        Some(format!("{CORE_PANIC_LOCATION_PREFIX} {file_name}:{line}"));
}

pub fn track_stream_connect_job() -> StreamConnectJobGuard {
    let tracked = is_embedded();
    if tracked {
        let mut jobs = STREAM_CONNECT_JOBS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *jobs = jobs
            .checked_add(1)
            .expect("stream-connect job tracking overflow");
    }
    StreamConnectJobGuard { tracked }
}

pub fn wait_for_stream_connect_jobs(timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    let mut jobs = STREAM_CONNECT_JOBS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    while *jobs > 0 {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return false;
        }
        let (next_jobs, wait_result) = STREAM_CONNECT_JOBS_FINISHED
            .wait_timeout(jobs, remaining)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        jobs = next_jobs;
        if wait_result.timed_out() && *jobs > 0 {
            return false;
        }
    }
    true
}

pub fn report_available_exit_countries(mut countries: Vec<String>) {
    if !is_embedded() {
        return;
    }
    countries.retain(|country| {
        country.len() == 2
            && country != "ZZ"
            && country
                .chars()
                .all(|character| character.is_ascii_uppercase())
    });
    countries.sort_unstable();
    countries.dedup();
    *AVAILABLE_EXIT_COUNTRIES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = countries;
}

fn add_bytes(counter: &AtomicU64, amount: usize) {
    if !is_embedded() {
        return;
    }
    let amount = u64::try_from(amount).unwrap_or(u64::MAX);
    let _ = counter.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |current| {
        Some(current.saturating_add(amount))
    });
}

/// Records clear client payload accepted for an outward mobile MASQ route.
pub fn report_bytes_up(amount: usize) {
    add_bytes(&BYTES_UP, amount);
}

/// Records clear response payload delivered from a mobile MASQ exit to the local client.
pub fn report_bytes_down(amount: usize) {
    add_bytes(&BYTES_DOWN, amount);
}

pub fn stop() {
    if !is_embedded() {
        return;
    }
    STOP_REQUESTED.store(true, Ordering::SeqCst);
    ACTOR_ARBITER_SHUTDOWN_REQUESTED.store(true, Ordering::SeqCst);
    // Drop the UI log target before actors begin shutting down. Late shutdown logs still reach the
    // process logger, but cannot recurse through a closed UiGateway.
    masq_lib::logger::clear_log_recipient();
    let actor_arbiters = ACTOR_ARBITERS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    for arbiter in actor_arbiters {
        arbiter.do_send(StopArbiter(0));
    }
    let system = SYSTEM
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    if let Some(system) = system {
        system.stop();
    }
}

pub fn snapshot() -> MobileRuntimeSnapshot {
    let proxy_port = PROXY_PORT.load(Ordering::SeqCst);
    let last_exit_code = LAST_EXIT_CODE.load(Ordering::SeqCst);
    MobileRuntimeSnapshot {
        started: STARTED.load(Ordering::SeqCst),
        stop_requested: STOP_REQUESTED.load(Ordering::SeqCst),
        proxy_port: if proxy_port == 0 {
            None
        } else {
            Some(proxy_port)
        },
        route_stage: ROUTE_STAGE.load(Ordering::SeqCst),
        route_hops: ROUTE_HOPS.load(Ordering::SeqCst),
        route_proof_generation: ROUTE_PROOF_GENERATION.load(Ordering::SeqCst),
        bytes_up: BYTES_UP.load(Ordering::SeqCst),
        bytes_down: BYTES_DOWN.load(Ordering::SeqCst),
        last_exit_code: if last_exit_code == NO_EXIT_CODE {
            None
        } else {
            Some(last_exit_code)
        },
        last_connection_error: LAST_CONNECTION_ERROR
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone(),
        available_exit_countries: AVAILABLE_EXIT_COUNTRIES
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone(),
    }
}

pub(crate) fn finish(exit_code: i32) {
    let _lifecycle = RUNTIME_LIFECYCLE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    masq_lib::logger::clear_log_recipient();
    STARTED.store(false, Ordering::SeqCst);
    STOP_REQUESTED.store(false, Ordering::SeqCst);
    PROXY_PORT.store(0, Ordering::SeqCst);
    ROUTE_STAGE.store(0, Ordering::SeqCst);
    ROUTE_HOPS.store(0, Ordering::SeqCst);
    ROUTE_PROOF_GENERATION.store(0, Ordering::SeqCst);
    reset_entry_handshake_milestone();
    LAST_EXIT_CODE.store(exit_code, Ordering::SeqCst);
    *SYSTEM
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    *NEIGHBORHOOD_UI
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    MIN_HOPS_PREFERENCE.store(0, Ordering::SeqCst);
    *NEIGHBORHOOD_CONFIG
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    *NEIGHBORHOOD_RETRY
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    *NEIGHBORHOOD_ROUTE_READINESS_RENEWAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    *PENDING_ENTRY_NODES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    *EXIT_PREFERENCE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    AVAILABLE_EXIT_COUNTRIES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
    EMBEDDED.store(false, Ordering::SeqCst);
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix::{Actor, Context, Handler};
    use std::sync::Arc;

    struct RouteReadinessRenewalRecorder {
        deliveries: Arc<AtomicUsize>,
    }

    impl Actor for RouteReadinessRenewalRecorder {
        type Context = Context<Self>;
    }

    impl Handler<RenewRouteReadinessLeaseMessage> for RouteReadinessRenewalRecorder {
        type Result = ();

        fn handle(
            &mut self,
            message: RenewRouteReadinessLeaseMessage,
            _context: &mut Self::Context,
        ) -> Self::Result {
            self.deliveries.fetch_add(1, Ordering::SeqCst);
            let _ = message
                .acknowledgement
                .try_send(runtime_epoch_is_current(message.mobile_runtime_epoch));
            System::current().stop();
        }
    }

    #[test]
    #[serial_test::serial]
    fn snapshot_tracks_an_embedded_consumer_without_exposing_a_serving_state() {
        prepare(44_443);
        mark_started();
        report_route_hops(3);
        report_route_stage(2);
        report_route_use_succeeded();

        let mut running_snapshot = snapshot();
        // Neighborhood actor tests run on independent Actix systems and can
        // report additional anonymous route-use heartbeats while this global
        // embedded-runtime fixture is active. Verify the presence of proof,
        // then normalize only that aggregate counter for the structural check.
        assert!(running_snapshot.route_proof_generation >= 1);
        running_snapshot.route_proof_generation = 1;
        assert_eq!(
            running_snapshot,
            MobileRuntimeSnapshot {
                started: true,
                stop_requested: false,
                proxy_port: Some(44_443),
                route_stage: 2,
                route_hops: 3,
                route_proof_generation: 1,
                bytes_up: 0,
                bytes_down: 0,
                last_exit_code: None,
                last_connection_error: None,
                available_exit_countries: vec![],
            }
        );

        finish(0);

        assert!(!is_embedded());
        assert_eq!(
            snapshot(),
            MobileRuntimeSnapshot {
                started: false,
                stop_requested: false,
                proxy_port: None,
                route_stage: 0,
                route_hops: 0,
                route_proof_generation: 0,
                bytes_up: 0,
                bytes_down: 0,
                last_exit_code: Some(0),
                last_connection_error: None,
                available_exit_countries: vec![],
            }
        );
    }

    #[test]
    #[serial_test::serial]
    fn route_proof_generation_is_monotone_and_saturates() {
        let counter = AtomicU64::new(0);
        advance_route_proof_generation(&counter);
        advance_route_proof_generation(&counter);
        assert_eq!(counter.load(Ordering::SeqCst), 2);

        counter.store(u64::MAX, Ordering::SeqCst);
        advance_route_proof_generation(&counter);
        assert_eq!(counter.load(Ordering::SeqCst), u64::MAX);
    }

    #[test]
    #[serial_test::serial]
    fn epoch_fenced_route_proof_never_mutates_a_reprepared_runtime() {
        prepare(44_443);
        mark_started();
        let old_epoch = current_runtime_epoch_for_test().unwrap();

        prepare(44_444);
        mark_started();
        let current_epoch = current_runtime_epoch_for_test().unwrap();
        assert_ne!(old_epoch, current_epoch);
        assert!(!report_route_use_succeeded_for_epoch(old_epoch));
        assert_eq!(snapshot().route_proof_generation, 0);
        assert!(report_route_use_succeeded_for_epoch(current_epoch));
        assert_eq!(snapshot().route_proof_generation, 1);
        finish(0);
    }

    #[test]
    #[serial_test::serial]
    fn route_readiness_renewal_requires_the_current_live_neighborhood_mailbox() {
        let system = System::new("route_readiness_renewal_mailbox");
        let deliveries = Arc::new(AtomicUsize::new(0));
        let recipient = RouteReadinessRenewalRecorder {
            deliveries: deliveries.clone(),
        }
        .start()
        .recipient::<RenewRouteReadinessLeaseMessage>();

        prepare(44_443);
        mark_started();
        assert!(!request_route_readiness_lease_renewal());
        register_neighborhood_route_readiness_renewal(recipient.clone());

        // A new runtime preparation fences the older actor recipient.
        prepare(44_444);
        mark_started();
        assert!(!request_route_readiness_lease_renewal());
        register_neighborhood_route_readiness_renewal(recipient);
        let renewal = thread::spawn(request_route_readiness_lease_renewal);
        system.run();
        assert!(renewal.join().unwrap());
        assert_eq!(deliveries.load(Ordering::SeqCst), 1);
        finish(0);
        assert!(!request_route_readiness_lease_renewal());
    }

    #[test]
    #[serial_test::serial]
    fn snapshot_exposes_the_latest_entry_connection_error() {
        prepare(44_443);
        mark_started();
        report_connection_error(MobileConnectionError::StreamConnectionFailed);

        assert_eq!(
            snapshot().last_connection_error.as_deref(),
            Some("Stream connection failed")
        );

        finish(0);
    }

    #[test]
    #[serial_test::serial]
    fn snapshot_tracks_mobile_payload_bytes_and_resets_them_for_a_new_run() {
        prepare(44_443);
        report_bytes_up(123);
        report_bytes_up(7);
        report_bytes_down(456);

        let active = snapshot();
        assert_eq!(active.bytes_up, 130);
        assert_eq!(active.bytes_down, 456);

        finish(0);
        prepare(44_444);
        let restarted = snapshot();
        assert_eq!(restarted.bytes_up, 0);
        assert_eq!(restarted.bytes_down, 0);
        finish(0);
    }

    #[test]
    #[serial_test::serial]
    fn stores_a_mobile_exit_country_preference_until_shutdown() {
        prepare(44_443);
        set_exit_preference(Some("BE"), false);

        assert_eq!(
            EXIT_PREFERENCE
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone(),
            Some(MobileExitPreference {
                country_code: Some("BE".to_owned()),
                fallback_routing: false,
            })
        );

        finish(0);
        assert!(EXIT_PREFERENCE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_none());
    }

    #[test]
    #[serial_test::serial]
    fn validates_and_queues_live_route_length_changes() {
        prepare(44_443);
        report_route_stage(2);

        assert!(set_min_hops(0).is_err());
        assert!(set_min_hops(7).is_err());
        set_min_hops(4).unwrap();

        assert_eq!(MIN_HOPS_PREFERENCE.load(Ordering::SeqCst), 4);
        assert_eq!(snapshot().route_stage, 1);
        finish(0);
    }

    #[test]
    #[serial_test::serial]
    fn reports_only_sorted_unique_iso_exit_countries() {
        prepare(44_443);
        report_available_exit_countries(vec![
            "US".to_owned(),
            "BE".to_owned(),
            "US".to_owned(),
            "ZZ".to_owned(),
            "invalid".to_owned(),
        ]);

        assert_eq!(snapshot().available_exit_countries, vec!["BE", "US"]);
        finish(0);
    }

    #[test]
    #[serial_test::serial]
    fn entry_handshake_milestones_are_monotone_and_reset_for_real_retries() {
        prepare(44_443);
        mark_started();

        report_entry_handshake_milestone(EntryHandshakeMilestone::DebutBytesWritten);
        report_entry_handshake_milestone(EntryHandshakeMilestone::TcpConnected);
        assert_eq!(
            entry_handshake_milestone(),
            EntryHandshakeMilestone::DebutBytesWritten
        );
        let (milestone, age, attempt_age) = entry_handshake_progress();
        assert_eq!(milestone, EntryHandshakeMilestone::DebutBytesWritten);
        assert!(age < Duration::from_secs(1));
        assert!(attempt_age < Duration::from_secs(1));

        retry_connection(&[]).expect_err("retry recipient is intentionally absent");
        assert_eq!(entry_handshake_milestone(), EntryHandshakeMilestone::None);
        finish(0);
    }

    #[test]
    #[serial_test::serial]
    fn stage_one_to_zero_resets_the_entry_handshake_milestone() {
        prepare(44_443);
        report_entry_handshake_milestone(EntryHandshakeMilestone::GossipAccepted);

        report_route_stage(0);
        assert_eq!(
            entry_handshake_milestone(),
            EntryHandshakeMilestone::GossipAccepted
        );
        report_route_stage(1);
        report_route_stage(0);
        assert_eq!(entry_handshake_milestone(), EntryHandshakeMilestone::None);
        finish(0);
    }

    #[test]
    #[serial_test::serial]
    fn stream_connect_jobs_are_bounded_by_an_acknowledged_wait() {
        prepare(44_443);
        let job = track_stream_connect_job();

        assert!(!wait_for_stream_connect_jobs(Duration::from_millis(1)));
        drop(job);
        assert!(wait_for_stream_connect_jobs(Duration::from_millis(1)));
        finish(0);
    }

    #[test]
    #[serial_test::serial]
    fn a_pass_loop_error_is_not_overwritten_by_a_later_transport_failure() {
        prepare(44_443);
        report_connection_error(MobileConnectionError::PassLoopFound);
        report_connection_error(MobileConnectionError::StreamConnectionFailed);

        assert_eq!(
            snapshot().last_connection_error.as_deref(),
            Some("E_ENTRY_GOSSIP_PASS_LOOP: The entry-node handshake encountered a pass loop")
        );
        finish(0);
    }

    #[test]
    #[serial_test::serial]
    fn terminal_entry_progress_is_not_overwritten_by_a_later_transport_failure() {
        prepare(44_443);
        report_connection_error(MobileConnectionError::NoEntryProgress);
        report_connection_error(MobileConnectionError::StreamConnectionFailed);

        assert_eq!(
            snapshot().last_connection_error.as_deref(),
            Some("E_ENTRY_NO_PROGRESS: All selected entry peers exhausted the initial handshake")
        );
        finish(0);
    }

    #[test]
    #[serial_test::serial]
    fn aggregate_terminal_progress_replaces_and_survives_per_peer_pass_loop_events() {
        prepare(44_443);
        report_connection_error(MobileConnectionError::PassLoopFound);
        report_connection_error(MobileConnectionError::NoEntryProgress);
        report_connection_error(MobileConnectionError::PassLoopFound);

        assert_eq!(
            snapshot().last_connection_error.as_deref(),
            Some("E_ENTRY_NO_PROGRESS: All selected entry peers exhausted the initial handshake")
        );
        finish(0);
    }

    #[test]
    #[serial_test::serial]
    fn a_proven_entry_peer_clears_only_stale_terminal_entry_errors() {
        prepare(44_443);
        report_connection_error(MobileConnectionError::NoEntryProgress);

        report_route_stage(1);

        assert_eq!(snapshot().last_connection_error, None);
        report_connection_error(MobileConnectionError::RouteLengthPreferenceFailed);
        report_route_stage(2);
        assert_eq!(
            snapshot().last_connection_error.as_deref(),
            Some("Route length could not be applied")
        );
        finish(0);
    }

    #[test]
    #[serial_test::serial]
    fn entry_attempt_age_is_independent_from_runtime_age_and_resets() {
        prepare(44_443);
        {
            let now = Instant::now();
            let mut progress = ENTRY_HANDSHAKE_PROGRESS
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            progress.attempt_started_at = Some(now - Duration::from_secs(40));
            progress.last_activity_at = Some(now - Duration::from_secs(3));
        }
        let (_, inactivity, attempt_age) = entry_handshake_progress();
        assert!(inactivity >= Duration::from_secs(3));
        assert!(inactivity < Duration::from_secs(4));
        assert!(attempt_age >= Duration::from_secs(40));
        assert!(attempt_age < Duration::from_secs(41));

        reset_entry_handshake_milestone();
        let (_, inactivity, attempt_age) = entry_handshake_progress();
        assert!(inactivity < Duration::from_secs(1));
        assert!(attempt_age < Duration::from_secs(1));
        finish(0);
    }

    #[test]
    #[serial_test::serial]
    fn panic_location_keeps_only_a_bounded_file_name_and_line() {
        prepare(44_443);

        report_panic_location("/private/person/stream handler.rs", 123);

        assert_eq!(
            snapshot().last_connection_error.as_deref(),
            Some("E_CORE_PANIC_LOCATION: streamhandler.rs:123")
        );
        finish(0);
    }

    #[test]
    #[serial_test::serial]
    fn panic_location_is_not_overwritten_by_later_connection_errors() {
        prepare(44_443);
        report_panic_location("/private/person/bootstrapper.rs", 351);

        report_connection_error(MobileConnectionError::StreamConnectionFailed);
        report_connection_error(MobileConnectionError::PassLoopFound);

        assert_eq!(
            snapshot().last_connection_error.as_deref(),
            Some("E_CORE_PANIC_LOCATION: bootstrapper.rs:351")
        );
        finish(0);
    }
}
