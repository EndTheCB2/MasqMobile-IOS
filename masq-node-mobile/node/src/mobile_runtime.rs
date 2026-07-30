// Copyright (c) 2026, MASQ (https://masq.ai) and/or its affiliates.
//
// Small process-local control surface for embedding the existing Node actor system in a mobile
// application. It deliberately exposes no serving controls: the mobile launcher is accepted only
// together with `--neighborhood-mode consume-only` by the configurator.

use crate::sub_lib::neighborhood::{ConfigChange, ConfigChangeMsg, Hops};
use crate::sub_lib::peer_actors::StartMessage;
use actix::msgs::StopArbiter;
use actix::{Addr, Arbiter, Recipient, System};
use masq_lib::messages::{CountryGroups, ToMessageBody, UiSetExitLocationRequest};
use masq_lib::ui_gateway::{MessageBody, NodeFromUiMessage};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU16, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const NO_EXIT_CODE: i32 = i32::MIN;

static EMBEDDED: AtomicBool = AtomicBool::new(false);
static STARTED: AtomicBool = AtomicBool::new(false);
static STOP_REQUESTED: AtomicBool = AtomicBool::new(false);
static PROXY_PORT: AtomicU16 = AtomicU16::new(0);
static ROUTE_STAGE: AtomicU8 = AtomicU8::new(0);
static ROUTE_HOPS: AtomicU8 = AtomicU8::new(0);
static ENTRY_HANDSHAKE_MILESTONE: AtomicU8 = AtomicU8::new(EntryHandshakeMilestone::None as u8);
static LAST_EXIT_CODE: AtomicI32 = AtomicI32::new(NO_EXIT_CODE);
static SYSTEM: Mutex<Option<System>> = Mutex::new(None);
static ACTOR_ARBITERS: Mutex<Vec<Addr<Arbiter>>> = Mutex::new(Vec::new());
static EXPECTED_ACTOR_ARBITERS: AtomicUsize = AtomicUsize::new(0);
static REGISTERED_ACTOR_ARBITERS: AtomicUsize = AtomicUsize::new(0);
static ACTOR_ARBITER_TRACKING_ACTIVE: AtomicBool = AtomicBool::new(false);
static ACTOR_ARBITER_SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);
static LAST_CONNECTION_ERROR: Mutex<Option<String>> = Mutex::new(None);
static AVAILABLE_EXIT_COUNTRIES: Mutex<Vec<String>> = Mutex::new(Vec::new());
static EXIT_PREFERENCE: Mutex<Option<MobileExitPreference>> = Mutex::new(None);
static NEIGHBORHOOD_UI: Mutex<Option<Recipient<NodeFromUiMessage>>> = Mutex::new(None);
static MIN_HOPS_PREFERENCE: AtomicU8 = AtomicU8::new(0);
static NEIGHBORHOOD_CONFIG: Mutex<Option<Recipient<ConfigChangeMsg>>> = Mutex::new(None);
static NEIGHBORHOOD_RETRY: Mutex<Option<Recipient<StartMessage>>> = Mutex::new(None);
static PENDING_ENTRY_NODES: Mutex<Option<Vec<String>>> = Mutex::new(None);
static STREAM_CONNECT_JOBS: Mutex<usize> = Mutex::new(0);
static STREAM_CONNECT_JOBS_FINISHED: Condvar = Condvar::new();

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MobileConnectionError {
    StreamConnectionFailed,
    PassLoopFound,
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
            Self::RouteLengthPreferenceFailed => "Route length could not be applied",
            Self::ExitCountryPreferenceFailed => "Exit-country preference could not be applied",
        }
    }

    fn takes_priority_over(self, current: Option<&str>) -> bool {
        self == Self::PassLoopFound
            || !current
                .map(|message| message.starts_with("E_ENTRY_GOSSIP_PASS_LOOP:"))
                .unwrap_or(false)
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

impl EntryHandshakeMilestone {
    fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::TcpConnected,
            2 => Self::DebutBytesWritten,
            3 => Self::InboundBytesReceived,
            4 => Self::GossipAccepted,
            _ => Self::None,
        }
    }
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MobileRuntimeSnapshot {
    pub started: bool,
    pub stop_requested: bool,
    pub proxy_port: Option<u16>,
    /// 0 = not connected, 1 = connected to a neighbor, 2 = usable route found.
    pub route_stage: u8,
    /// Actual outward MASQ hops in the most recently selected route.
    pub route_hops: u8,
    pub last_exit_code: Option<i32>,
    pub last_connection_error: Option<String>,
    pub available_exit_countries: Vec<String>,
}

/// Runs an embedded Node after the caller has prepared the runtime and applied
/// its preferences. Keeping preparation outside this function prevents actor
/// startup from clearing preferences that were queued just before spawning.
pub fn run_embedded(args: &[String]) -> i32 {
    let exit_code = catch_unwind(AssertUnwindSafe(|| {
        crate::sub_lib::main_tools::main_with_args(args)
    }))
    .unwrap_or(101);
    finish(exit_code);
    exit_code
}

pub fn prepare(proxy_port: u16) {
    // A previous embedded actor system may have exited in this same app process. Its UI Gateway
    // recipient is no longer valid and must not receive startup logs from the next system.
    masq_lib::logger::clear_log_recipient();
    EMBEDDED.store(true, Ordering::SeqCst);
    STARTED.store(false, Ordering::SeqCst);
    STOP_REQUESTED.store(false, Ordering::SeqCst);
    PROXY_PORT.store(proxy_port, Ordering::SeqCst);
    ROUTE_STAGE.store(0, Ordering::SeqCst);
    ROUTE_HOPS.store(0, Ordering::SeqCst);
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
    *PENDING_ENTRY_NODES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
}

pub fn is_embedded() -> bool {
    EMBEDDED.load(Ordering::SeqCst)
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
            apply_exit_preference();
        }
    }
}

pub fn report_entry_handshake_milestone(milestone: EntryHandshakeMilestone) {
    if is_embedded() {
        ENTRY_HANDSHAKE_MILESTONE.fetch_max(milestone as u8, Ordering::SeqCst);
    }
}

pub fn entry_handshake_milestone() -> EntryHandshakeMilestone {
    EntryHandshakeMilestone::from_u8(ENTRY_HANDSHAKE_MILESTONE.load(Ordering::SeqCst))
}

fn reset_entry_handshake_milestone() {
    ENTRY_HANDSHAKE_MILESTONE.store(EntryHandshakeMilestone::None as u8, Ordering::SeqCst);
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
    masq_lib::logger::clear_log_recipient();
    STARTED.store(false, Ordering::SeqCst);
    STOP_REQUESTED.store(false, Ordering::SeqCst);
    PROXY_PORT.store(0, Ordering::SeqCst);
    ROUTE_STAGE.store(0, Ordering::SeqCst);
    ROUTE_HOPS.store(0, Ordering::SeqCst);
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

    #[test]
    #[serial_test::serial]
    fn snapshot_tracks_an_embedded_consumer_without_exposing_a_serving_state() {
        prepare(44_443);
        mark_started();
        report_route_hops(3);
        report_route_stage(2);

        assert_eq!(
            snapshot(),
            MobileRuntimeSnapshot {
                started: true,
                stop_requested: false,
                proxy_port: Some(44_443),
                route_stage: 2,
                route_hops: 3,
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
                last_exit_code: Some(0),
                last_connection_error: None,
                available_exit_countries: vec![],
            }
        );
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
}
