use serde::Serialize;

#[cfg(feature = "node-engine")]
use std::{
    io::{self, Read, Write},
    net::TcpStream,
    thread,
    time::{Duration, Instant},
};

use crate::config::{Chain, MobileConfig};
#[cfg(feature = "node-engine")]
use crate::engine::{EngineHandle, RetryConnectionOutcome};
use crate::wallet::WalletMaterial;

#[cfg(feature = "node-engine")]
use node_lib::mobile_debt_settlement::PreparedDebtSettlement;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
#[allow(dead_code)]
pub enum Phase {
    Unconfigured,
    Ready,
    Connecting,
    Connected,
    Paused,
    Stopping,
    Blocked,
    Error,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CoreStatus<'a> {
    phase: Phase,
    engine_available: bool,
    engine_generation: u64,
    proxy_enabled: bool,
    proxy_port: Option<u16>,
    chain: Option<Chain>,
    wallet_address: Option<&'a str>,
    connected_neighbors: usize,
    route_stage: u8,
    route_hops: usize,
    route_proof_generation: u64,
    min_hops: u8,
    exit_country: Option<&'a str>,
    exit_country_fallback: bool,
    available_exit_countries: &'a [String],
    bytes_up: u64,
    bytes_down: u64,
    last_error: Option<&'a str>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct RouteProofRefreshOutcome {
    attempted: bool,
    succeeded: bool,
    error_code: Option<&'static str>,
}

impl RouteProofRefreshOutcome {
    const fn succeeded() -> Self {
        Self {
            attempted: true,
            succeeded: true,
            error_code: None,
        }
    }

    const fn failed() -> Self {
        Self {
            attempted: true,
            succeeded: false,
            error_code: Some("E_PRIVATE_ROUTE_REFRESH_FAILED"),
        }
    }

    const fn not_ready() -> Self {
        Self {
            attempted: false,
            succeeded: false,
            error_code: Some("E_PRIVATE_ROUTE_REFRESH_NOT_READY"),
        }
    }

    #[cfg(not(feature = "node-engine"))]
    const fn unavailable() -> Self {
        Self {
            attempted: false,
            succeeded: false,
            error_code: Some("E_PRIVATE_ROUTE_REFRESH_UNAVAILABLE"),
        }
    }
}

/// Exact process-local identity of the engine whose healthy route is being
/// refreshed. The socket probe deliberately owns only this copy, never a
/// borrow of `MobileCore`, so callers can release the global core mutex while
/// waiting for CONNECT, TLS and the remote response.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RouteProofRefreshTicket {
    engine_generation: u64,
    proxy_port: u16,
}

impl RouteProofRefreshTicket {
    pub(crate) const fn proxy_port(self) -> u16 {
        self.proxy_port
    }
}

#[cfg(feature = "node-engine")]
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PublicDebtSettlementQuote<'a> {
    quote_id: &'a str,
    created_at_unix_seconds: u64,
    expires_at_unix_seconds: u64,
    total_masq_wei: String,
    estimated_l2_fee_wei: String,
    masq_balance_wei: String,
    base_eth_balance_wei: String,
    creditor_count: usize,
    has_more_creditors: bool,
    fee_estimate_includes_l1_data_fee: bool,
    requires_device_authentication: bool,
    requires_explicit_confirmation: bool,
}

pub struct MobileCore {
    config: Option<MobileConfig>,
    wallet: Option<WalletMaterial>,
    phase: Phase,
    engine_generation: u64,
    proxy_enabled: bool,
    proxy_port: Option<u16>,
    connected_neighbors: usize,
    route_stage: u8,
    route_hops: usize,
    route_proof_generation: u64,
    bytes_up: u64,
    bytes_down: u64,
    last_error: Option<String>,
    available_exit_countries: Vec<String>,
    #[cfg(feature = "node-engine")]
    engine: Option<EngineHandle>,
    #[cfg(feature = "node-engine")]
    prepared_debt_settlement: Option<PreparedDebtSettlement>,
}

impl Default for MobileCore {
    fn default() -> Self {
        Self {
            config: None,
            wallet: None,
            phase: Phase::Unconfigured,
            engine_generation: 0,
            proxy_enabled: false,
            proxy_port: None,
            connected_neighbors: 0,
            route_stage: 0,
            route_hops: 0,
            route_proof_generation: 0,
            bytes_up: 0,
            bytes_down: 0,
            last_error: None,
            available_exit_countries: Vec::new(),
            #[cfg(feature = "node-engine")]
            engine: None,
            #[cfg(feature = "node-engine")]
            prepared_debt_settlement: None,
        }
    }
}

impl MobileCore {
    pub fn configure(&mut self, json: &str) -> Result<(), String> {
        let config = MobileConfig::parse(json)?;
        #[cfg(feature = "node-engine")]
        if self.engine.is_some() {
            let current = self
                .config
                .as_ref()
                .ok_or_else(|| "The running MASQ profile is missing.".to_owned())?;
            let only_entry_nodes_changed = current.chain == config.chain
                && current.rpc_url == config.rpc_url
                && current.min_hops == config.min_hops
                && current.exit_country == config.exit_country
                && current.exit_country_fallback == config.exit_country_fallback
                && current.data_directory == config.data_directory;
            let entry_nodes_changed = current.neighbors != config.neighbors;
            if !only_entry_nodes_changed {
                return self
                    .fail("Fully restart the app before changing the chain or blockchain RPC.");
            }
            if entry_retry_requires_fresh_runtime(
                self.phase,
                entry_nodes_changed,
                self.last_error.as_deref(),
            ) {
                if !self.stop_engine_for_state_reset() {
                    return Err(
                        "The previous MASQ peer session could not be stopped for a safe retry."
                            .to_owned(),
                    );
                }
                self.proxy_enabled = false;
                self.proxy_port = None;
                self.connected_neighbors = 0;
                self.route_stage = 0;
                self.route_hops = 0;
                self.route_proof_generation = 0;
            }
        }
        self.config = Some(config);
        #[cfg(feature = "node-engine")]
        {
            self.prepared_debt_settlement = None;
        }
        self.phase = if self.wallet.is_some() {
            Phase::Ready
        } else {
            Phase::Unconfigured
        };
        self.last_error = None;
        self.available_exit_countries.clear();
        Ok(())
    }

    pub fn import_wallet(&mut self, private_key: &str) -> Result<(), String> {
        self.wallet = Some(WalletMaterial::import(private_key)?);
        #[cfg(feature = "node-engine")]
        {
            self.prepared_debt_settlement = None;
        }
        self.phase = if self.config.is_some() {
            Phase::Ready
        } else {
            Phase::Unconfigured
        };
        self.last_error = None;
        self.available_exit_countries.clear();
        Ok(())
    }

    pub fn update_min_hops(&mut self, min_hops: u8) -> Result<(), String> {
        if !(1..=6).contains(&min_hops) {
            return self.fail("Choose between one and six MASQ hops.");
        }
        if self.config.is_none() {
            return self.fail("Configure the MASQ network profile first.");
        }
        #[cfg(feature = "node-engine")]
        if let Some(engine) = self.engine.as_mut() {
            node_lib::mobile_runtime::set_min_hops(min_hops)?;
            engine.set_min_hops(min_hops);
        }
        self.config
            .as_mut()
            .expect("configuration checked above")
            .min_hops = min_hops;
        self.route_stage = self.route_stage.min(1);
        self.route_hops = 0;
        self.last_error = None;
        self.available_exit_countries.clear();
        Ok(())
    }

    pub fn start(&mut self) -> Result<(), String> {
        if self.config.is_none() || self.wallet.is_none() {
            return self.fail("Configure the chain, entry nodes, and consumer wallet first.");
        }
        if !engine_available() {
            self.phase = Phase::Blocked;
            self.last_error =
                Some("The MASQ Node actor adapter is not linked to this mobile core.".to_owned());
            return Ok(());
        }

        #[cfg(feature = "node-engine")]
        {
            if matches!(self.phase, Phase::Connected) {
                return Ok(());
            }
            if let Some(engine) = self.engine.as_mut() {
                let retry_outcome = engine.retry_connection(
                    &self
                        .config
                        .as_ref()
                        .expect("configuration checked above")
                        .neighbors,
                )?;
                match retry_outcome {
                    RetryConnectionOutcome::RetriedInPlace => {
                        self.phase = Phase::Connecting;
                        self.proxy_enabled = false;
                        self.connected_neighbors = 0;
                        self.route_stage = 0;
                        self.route_hops = 0;
                        self.route_proof_generation = 0;
                        self.last_error = None;
                        self.refresh_engine_status();
                        return Ok(());
                    }
                    RetryConnectionOutcome::RestartRequired => {
                        // The old JoinHandle was already observed as finished
                        // and reaped. A live embedded runtime is never stopped
                        // merely because refreshed descriptors differ.
                        self.engine.take();
                    }
                }
            }
            let next_engine_generation = self
                .engine_generation
                .checked_add(1)
                .ok_or_else(|| "The MASQ engine generation is exhausted.".to_owned())?;
            let engine = match EngineHandle::start(
                self.config.as_ref().expect("configuration checked above"),
                self.wallet.as_ref().expect("wallet checked above"),
            ) {
                Ok(engine) => engine,
                Err(error) => return self.fail(&error),
            };
            self.engine = Some(engine);
            self.engine_generation = next_engine_generation;
        }
        self.phase = Phase::Connecting;
        self.proxy_enabled = false;
        self.route_stage = 0;
        self.route_proof_generation = 0;
        Ok(())
    }

    pub fn stop(&mut self) {
        #[cfg(feature = "node-engine")]
        if self.engine.is_some() {
            // Normal pause keeps the consume-only mesh warm for a fast resume. Explicit direct
            // browsing uses the separate full shutdown transition; the embedded runtime clears
            // process-global lifecycle state so MASQ can later reconnect in this same app process.
            self.phase = Phase::Paused;
            self.proxy_enabled = false;
            return;
        }
        self.phase = if self.config.is_some() && self.wallet.is_some() {
            Phase::Ready
        } else {
            Phase::Unconfigured
        };
        self.proxy_enabled = false;
        self.proxy_port = None;
        self.connected_neighbors = 0;
        self.route_stage = 0;
        self.route_hops = 0;
        self.route_proof_generation = 0;
        self.last_error = None;
        self.available_exit_countries.clear();
    }

    pub fn shutdown(&mut self) {
        #[cfg(feature = "node-engine")]
        if let Some(mut engine) = self.engine.take() {
            // Returning Ready is the acknowledgement that no MASQ peer mesh remains active.
            // A stuck actor system is retained and reported as an error so Direct stays blocked.
            if let Err(error) = engine.stop_with_timeout(std::time::Duration::from_secs(10)) {
                self.engine = Some(engine);
                self.phase = Phase::Error;
                self.proxy_enabled = false;
                self.last_error = Some(error);
                return;
            }
        }
        self.phase = if self.config.is_some() && self.wallet.is_some() {
            Phase::Ready
        } else {
            Phase::Unconfigured
        };
        self.proxy_enabled = false;
        self.proxy_port = None;
        self.connected_neighbors = 0;
        self.route_stage = 0;
        self.route_hops = 0;
        self.route_proof_generation = 0;
        self.last_error = None;
        self.available_exit_countries.clear();
    }

    pub fn reset(&mut self) {
        #[cfg(feature = "node-engine")]
        if !self.stop_engine_for_state_reset() {
            return;
        }
        *self = Self::default();
    }

    pub fn reset_network_profile(&mut self) {
        #[cfg(feature = "node-engine")]
        if !self.stop_engine_for_state_reset() {
            return;
        }
        self.config = None;
        self.phase = Phase::Unconfigured;
        self.proxy_enabled = false;
        self.proxy_port = None;
        self.connected_neighbors = 0;
        self.route_stage = 0;
        self.route_hops = 0;
        self.route_proof_generation = 0;
        self.last_error = None;
        self.available_exit_countries.clear();
    }

    pub fn remove_wallet(&mut self) {
        #[cfg(feature = "node-engine")]
        if !self.stop_engine_for_state_reset() {
            return;
        }
        self.wallet = None;
        self.phase = Phase::Unconfigured;
        self.proxy_enabled = false;
        self.proxy_port = None;
        self.connected_neighbors = 0;
        self.route_stage = 0;
        self.route_hops = 0;
        self.route_proof_generation = 0;
        self.last_error = None;
        self.available_exit_countries.clear();
    }

    #[cfg(feature = "node-engine")]
    fn stop_engine_for_state_reset(&mut self) -> bool {
        let Some(mut engine) = self.engine.take() else {
            return true;
        };
        match engine.stop_with_timeout(std::time::Duration::from_secs(10)) {
            Ok(()) => true,
            Err(error) => {
                self.engine = Some(engine);
                self.phase = Phase::Error;
                self.proxy_enabled = false;
                self.last_error = Some(error);
                false
            }
        }
    }

    fn has_healthy_route_for_refresh(&self) -> bool {
        matches!(self.phase, Phase::Connected)
            && self.engine_generation > 0
            && self.connected_neighbors > 0
            && self.route_stage >= 2
            && self.proxy_port.is_some()
    }

    fn current_route_proof_refresh_ticket(&self) -> Option<RouteProofRefreshTicket> {
        self.has_healthy_route_for_refresh()
            .then(|| RouteProofRefreshTicket {
                engine_generation: self.engine_generation,
                proxy_port: self
                    .proxy_port
                    .expect("a healthy route always has a local proxy port"),
            })
    }

    /// Snapshots the precise engine identity for a scheduled route refresh.
    /// The caller must drop its core lock immediately after this returns.
    pub(crate) fn begin_route_proof_refresh(&mut self) -> Option<RouteProofRefreshTicket> {
        self.refresh_engine_status();
        self.current_route_proof_refresh_ticket()
    }

    fn route_proof_refresh_ticket_is_current(&self, ticket: RouteProofRefreshTicket) -> bool {
        self.current_route_proof_refresh_ticket() == Some(ticket)
    }

    fn status_json_without_refresh(&self) -> String {
        let status = CoreStatus {
            phase: self.phase,
            engine_available: engine_available(),
            engine_generation: self.engine_generation,
            proxy_enabled: self.proxy_enabled,
            proxy_port: self.proxy_port,
            chain: self.config.as_ref().map(|config| config.chain),
            wallet_address: self.wallet.as_ref().map(WalletMaterial::address),
            connected_neighbors: self.connected_neighbors,
            route_stage: self.route_stage,
            route_hops: self.route_hops,
            route_proof_generation: self.route_proof_generation,
            min_hops: self
                .config
                .as_ref()
                .map(|config| config.min_hops)
                .unwrap_or(1),
            exit_country: self
                .config
                .as_ref()
                .and_then(|config| config.exit_country.as_deref()),
            exit_country_fallback: self
                .config
                .as_ref()
                .map(|config| config.exit_country_fallback)
                .unwrap_or(true),
            available_exit_countries: &self.available_exit_countries,
            bytes_up: self.bytes_up,
            bytes_down: self.bytes_down,
            last_error: self.last_error.as_deref(),
        };
        serde_json::to_string(&status).expect("CoreStatus serialization is infallible")
    }

    fn status_json_with_route_proof_refresh(&self, outcome: RouteProofRefreshOutcome) -> String {
        let mut status: serde_json::Value =
            serde_json::from_str(&self.status_json_without_refresh())
                .expect("CoreStatus serialization must remain valid JSON");
        status["routeProofRefresh"] = serde_json::to_value(outcome)
            .expect("RouteProofRefreshOutcome serialization is infallible");
        serde_json::to_string(&status)
            .expect("route-proof refresh status serialization is infallible")
    }

    pub(crate) fn route_proof_refresh_not_ready_json(&self) -> String {
        self.status_json_with_route_proof_refresh(RouteProofRefreshOutcome::not_ready())
    }

    /// Completes a periodic refresh after the caller has reacquired the core
    /// lock. A result for an old engine generation or proxy port is discarded
    /// without refreshing, restoring or otherwise mutating the current core.
    pub(crate) fn complete_route_proof_refresh(
        &mut self,
        ticket: RouteProofRefreshTicket,
        probe_result: Result<(), String>,
    ) -> String {
        self.complete_route_proof_refresh_with(ticket, probe_result, |_| {
            #[cfg(feature = "node-engine")]
            node_lib::mobile_runtime::report_route_stage(2);
        })
    }

    fn complete_route_proof_refresh_with(
        &mut self,
        ticket: RouteProofRefreshTicket,
        probe_result: Result<(), String>,
        report_current_route: impl FnOnce(&mut MobileCore),
    ) -> String {
        self.complete_route_proof_refresh_with_hooks(
            ticket,
            probe_result,
            |core| core.refresh_engine_status(),
            report_current_route,
        )
    }

    fn complete_route_proof_refresh_with_hooks(
        &mut self,
        ticket: RouteProofRefreshTicket,
        probe_result: Result<(), String>,
        mut refresh_current_engine: impl FnMut(&mut MobileCore),
        report_current_route: impl FnOnce(&mut MobileCore),
    ) -> String {
        // The socket probe ran without CORE locked. Import the live engine
        // snapshot before trusting the cached ticket on both the success and
        // failure paths. Otherwise an engine that stopped or lost its route
        // during the probe could briefly be published as still connected.
        refresh_current_engine(self);
        if !self.route_proof_refresh_ticket_is_current(ticket) {
            return self.route_proof_refresh_not_ready_json();
        }

        let outcome = match probe_result {
            Err(_) => {
                // A scheduled keepalive is advisory. Its endpoint, TLS handshake, or
                // deadline can fail transiently while the already-proven MASQ route is
                // still carrying traffic. Never turn that single observation into a
                // global core error or revoke the browser/VPN route.
                RouteProofRefreshOutcome::failed()
            }
            Ok(()) => {
                // A correlated proxy response normally advances the actor runtime's
                // route-use heartbeat before this point. First import and validate
                // that state. Only then preserve the old explicit stage report for
                // this exact engine; a stale probe must never report into a newer
                // process-global runtime. Re-import and revalidate once more before
                // publishing success.
                report_current_route(self);
                refresh_current_engine(self);
                if !self.route_proof_refresh_ticket_is_current(ticket) {
                    return self.route_proof_refresh_not_ready_json();
                }
                self.last_error = None;
                RouteProofRefreshOutcome::succeeded()
            }
        };
        self.status_json_with_route_proof_refresh(outcome)
    }

    #[cfg(test)]
    pub(crate) fn healthy_for_route_refresh_test(engine_generation: u64, proxy_port: u16) -> Self {
        let mut core = Self::default();
        core.phase = Phase::Connected;
        core.engine_generation = engine_generation;
        core.proxy_enabled = true;
        core.proxy_port = Some(proxy_port);
        core.connected_neighbors = 1;
        core.route_stage = 2;
        core.route_hops = 3;
        core.route_proof_generation = 11;
        core
    }

    #[cfg(not(feature = "node-engine"))]
    pub(crate) fn route_proof_refresh_unavailable_json(&mut self) -> String {
        self.refresh_engine_status();
        self.status_json_with_route_proof_refresh(RouteProofRefreshOutcome::unavailable())
    }

    #[cfg(feature = "node-engine")]
    pub fn preflight_proxy(&mut self) -> Result<(), String> {
        if self.connected_neighbors == 0 || self.route_stage == 0 {
            return Err(
                "Connect to a MASQ entry peer before testing the private route.".to_owned(),
            );
        }
        self.probe_private_route()
    }

    /// Returns an operation-shaped error snapshot without poisoning the still-live entry session.
    /// A first exit-route proof can race topology gossip; JavaScript may safely retry it within the
    /// existing absolute connection deadline.
    #[cfg(feature = "node-engine")]
    pub fn preflight_proxy_status_json(&mut self) -> String {
        self.preflight_proxy_status_json_with(MobileCore::preflight_proxy)
    }

    #[cfg(feature = "node-engine")]
    fn preflight_proxy_status_json_with(
        &mut self,
        probe: impl FnOnce(&mut MobileCore) -> Result<(), String>,
    ) -> String {
        match probe(self) {
            Ok(()) => self.status_json(),
            Err(_) => self.status_json_with_transient_error(
                "E_PRIVATE_ROUTE_FAILED: MASQ could not yet prove an end-to-end private exit route.",
            ),
        }
    }

    #[cfg(not(feature = "node-engine"))]
    pub fn preflight_proxy_status_json(&mut self) -> String {
        self.status_json_with_transient_error(
            "E_PRIVATE_ROUTE_FAILED: The native MASQ Node actor adapter is unavailable.",
        )
    }

    fn status_json_with_transient_error(&mut self, error: &str) -> String {
        let mut status: serde_json::Value = serde_json::from_str(&self.status_json())
            .expect("CoreStatus serialization must remain valid JSON");
        status["phase"] = serde_json::json!("error");
        status["proxyEnabled"] = serde_json::json!(false);
        status["lastError"] = serde_json::json!(error);
        serde_json::to_string(&status)
            .expect("transient preflight status serialization is infallible")
    }

    #[cfg(feature = "node-engine")]
    fn probe_private_route(&mut self) -> Result<(), String> {
        let port = self
            .proxy_port
            .ok_or_else(|| "The local MASQ proxy has no port.".to_owned())?;
        Self::probe_private_route_for_refresh(port)?;

        node_lib::mobile_runtime::report_route_stage(2);
        self.refresh_engine_status();
        self.last_error = None;
        Ok(())
    }

    /// Runs only the bounded network I/O portion of a route proof. In
    /// particular, this function neither borrows `MobileCore` nor directly
    /// writes process-wide MASQ runtime state, which makes it safe to call
    /// without CORE's mutex being held. The proxied response can still update
    /// normal actor telemetry, which is imported only after ticket validation.
    #[cfg(feature = "node-engine")]
    pub(crate) fn probe_private_route_for_refresh(port: u16) -> Result<(), String> {
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(ROUTE_PROBE_TIMEOUT_SECONDS))
            .ok_or_else(|| "The MASQ route-test deadline could not be created.".to_owned())?;
        loop {
            let last_error = match Self::probe_private_route_once(port, deadline) {
                Ok(()) => break,
                Err(error) => error,
            };
            let remaining = match remaining_probe_time(deadline) {
                Ok(remaining) => remaining,
                Err(_) => return Err(last_error),
            };
            if remaining <= ROUTE_PROBE_RETRY_DELAY {
                return Err(last_error);
            }
            thread::sleep(ROUTE_PROBE_RETRY_DELAY);
        }
        Ok(())
    }

    #[cfg(feature = "node-engine")]
    fn probe_private_route_once(port: u16, deadline: Instant) -> Result<(), String> {
        use std::net::{Ipv4Addr, SocketAddrV4};

        let connect_timeout = remaining_probe_time(deadline)
            .map_err(|_| "The MASQ route test timed out before it could start.".to_owned())?;
        let stream = TcpStream::connect_timeout(
            &SocketAddrV4::new(Ipv4Addr::LOCALHOST, port).into(),
            connect_timeout,
        )
        .map_err(|error| format!("The local MASQ proxy could not be reached: {error}"))?;
        let mut stream = DeadlineStream::new(stream, deadline);
        stream
            .write_all(b"CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n")
            .map_err(|error| format!("The MASQ route test could not be sent: {error}"))?;
        let connect_header = read_bounded_http_header(&mut stream, 4096).map_err(|error| {
            format!("The local MASQ proxy did not accept the route test: {error}")
        })?;
        if http_status_code(&connect_header) != Some(200) {
            return Err("The local MASQ proxy rejected the private route test.".to_owned());
        }

        // CONNECT 200 is only the local proxy's acknowledgement. Complete a
        // TLS handshake and an encrypted HEAD request so READY requires bytes
        // from the remote origin through the MASQ exit route. Certificate
        // validation is deliberately not the purpose of this connectivity
        // probe; the embedded browser performs normal WebView validation.
        let connector = native_tls::TlsConnector::builder()
            .danger_accept_invalid_certs(true)
            .danger_accept_invalid_hostnames(true)
            .build()
            .map_err(|_| "The MASQ TLS route test could not be prepared.".to_owned())?;
        let mut tls = connector
            .connect("example.com", stream)
            .map_err(|_| "The MASQ exit route did not complete a TLS handshake.".to_owned())?;
        tls.write_all(b"HEAD / HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n\r\n")
            .map_err(|_| "The encrypted MASQ route test could not be sent.".to_owned())?;
        let origin_header = read_bounded_http_header(&mut tls, 8192)
            .map_err(|_| "The MASQ exit route returned no encrypted HTTP response.".to_owned())?;
        if !matches!(http_status_code(&origin_header), Some(100..=599)) {
            return Err("The MASQ exit route returned an invalid encrypted response.".to_owned());
        }

        Ok(())
    }

    #[cfg(not(feature = "node-engine"))]
    pub fn preflight_proxy(&mut self) -> Result<(), String> {
        self.fail("The native MASQ Node actor adapter is unavailable.")
    }

    pub fn set_proxy_enabled(&mut self, enabled: bool) -> Result<(), String> {
        if enabled && !matches!(self.phase, Phase::Connected) {
            return self.fail("The browser proxy requires a connected MASQ route.");
        }
        self.proxy_enabled = enabled;
        Ok(())
    }

    #[cfg(feature = "node-engine")]
    pub fn debt_summary_json(&self) -> Result<String, String> {
        let data_directory = self.data_directory()?;
        let summary = node_lib::mobile_debt_settlement::debt_summary(data_directory)?;
        serde_json::to_string(&summary)
            .map_err(|_| "The MASQ debt summary could not be encoded.".to_owned())
    }

    #[cfg(not(feature = "node-engine"))]
    pub fn debt_summary_json(&self) -> Result<String, String> {
        Err("The native MASQ accounting core is unavailable.".to_owned())
    }

    #[cfg(feature = "node-engine")]
    pub fn prepare_debt_settlement_json(&mut self) -> Result<String, String> {
        let config = self
            .config
            .as_ref()
            .ok_or_else(|| "Configure the MASQ network profile first.".to_owned())?;
        let wallet = self
            .wallet
            .as_ref()
            .ok_or_else(|| "Import the consumer wallet before settling debts.".to_owned())?;
        let data_directory = config
            .data_directory
            .as_deref()
            .ok_or_else(|| "The protected MASQ data directory is unavailable.".to_owned())?;
        let secret = wallet.private_key_bytes();
        let prepared = node_lib::mobile_debt_settlement::prepare_debt_settlement(
            std::path::Path::new(data_directory),
            &config.rpc_url,
            config.chain.identifier(),
            secret.as_slice(),
        )?;
        let created_at_unix_seconds = unix_seconds(prepared.created_at)?;
        let expires_at_unix_seconds = unix_seconds(prepared.expires_at)?;
        let public = PublicDebtSettlementQuote {
            quote_id: &prepared.quote_id,
            created_at_unix_seconds,
            expires_at_unix_seconds,
            total_masq_wei: prepared.total_masq_wei.to_string(),
            estimated_l2_fee_wei: prepared.estimated_l2_fee_wei.to_string(),
            masq_balance_wei: prepared.masq_balance_wei.to_string(),
            base_eth_balance_wei: prepared.base_eth_balance_wei.to_string(),
            creditor_count: prepared.creditor_count,
            has_more_creditors: prepared.has_more_creditors,
            fee_estimate_includes_l1_data_fee: false,
            requires_device_authentication: false,
            requires_explicit_confirmation: true,
        };
        let json = serde_json::to_string(&public)
            .map_err(|_| "The settlement quote could not be encoded.".to_owned())?;
        self.prepared_debt_settlement = Some(prepared);
        Ok(json)
    }

    #[cfg(not(feature = "node-engine"))]
    pub fn prepare_debt_settlement_json(&mut self) -> Result<String, String> {
        Err("The native MASQ accounting core is unavailable.".to_owned())
    }

    #[cfg(feature = "node-engine")]
    pub fn confirm_debt_settlement_json(
        &mut self,
        quote_id: &str,
        maximum_masq_wei: &str,
        maximum_estimated_l2_fee_wei: &str,
    ) -> Result<String, String> {
        let prepared = self
            .prepared_debt_settlement
            .as_ref()
            .filter(|prepared| constant_time_text_eq(&prepared.quote_id, quote_id))
            .cloned()
            .ok_or_else(|| "Review a current MASQ debt quote before settling.".to_owned())?;
        let maximum_masq_wei = parse_wei_limit(maximum_masq_wei, "MASQ amount")?;
        let maximum_estimated_l2_fee_wei =
            parse_wei_limit(maximum_estimated_l2_fee_wei, "estimated Base network fee")?;
        let config = self
            .config
            .as_ref()
            .cloned()
            .ok_or_else(|| "Configure the MASQ network profile first.".to_owned())?;
        let wallet_secret = self
            .wallet
            .as_ref()
            .ok_or_else(|| "Import the consumer wallet before settling debts.".to_owned())?
            .private_key_bytes();
        let data_directory = config
            .data_directory
            .as_deref()
            .ok_or_else(|| "The protected MASQ data directory is unavailable.".to_owned())?;

        let should_restart = self.engine.is_some();
        if let Some(mut engine) = self.engine.take() {
            self.phase = Phase::Stopping;
            self.proxy_enabled = false;
            if let Err(error) = engine.stop_with_timeout(Duration::from_secs(10)) {
                self.engine = Some(engine);
                self.phase = Phase::Error;
                self.last_error = Some(error.clone());
                return Err(error);
            }
            self.proxy_port = None;
            self.connected_neighbors = 0;
            self.route_stage = 0;
            self.route_hops = 0;
            self.route_proof_generation = 0;
        }

        let result = node_lib::mobile_debt_settlement::submit_prepared_debt_settlement(
            std::path::Path::new(data_directory),
            &config.rpc_url,
            config.chain.identifier(),
            wallet_secret.as_slice(),
            &prepared,
            maximum_masq_wei,
            maximum_estimated_l2_fee_wei,
        );
        if result.is_ok() {
            self.prepared_debt_settlement = None;
        }
        if should_restart {
            let _ = self.start();
        } else {
            self.phase = Phase::Ready;
        }
        let status = result?;
        serde_json::to_string(&status)
            .map_err(|_| "The settlement result could not be encoded.".to_owned())
    }

    #[cfg(not(feature = "node-engine"))]
    pub fn confirm_debt_settlement_json(
        &mut self,
        _quote_id: &str,
        _maximum_masq_wei: &str,
        _maximum_estimated_l2_fee_wei: &str,
    ) -> Result<String, String> {
        Err("The native MASQ accounting core is unavailable.".to_owned())
    }

    #[cfg(feature = "node-engine")]
    pub fn retry_debt_settlement_json(&mut self) -> Result<String, String> {
        let config = self
            .config
            .as_ref()
            .cloned()
            .ok_or_else(|| "Configure the MASQ network profile first.".to_owned())?;
        let data_directory = config
            .data_directory
            .as_deref()
            .ok_or_else(|| "The protected MASQ data directory is unavailable.".to_owned())?;
        let should_restart = self.engine.is_some();
        if let Some(mut engine) = self.engine.take() {
            self.phase = Phase::Stopping;
            self.proxy_enabled = false;
            if let Err(error) = engine.stop_with_timeout(Duration::from_secs(10)) {
                self.engine = Some(engine);
                self.phase = Phase::Error;
                self.last_error = Some(error.clone());
                return Err(error);
            }
            self.proxy_port = None;
            self.connected_neighbors = 0;
            self.route_stage = 0;
            self.route_hops = 0;
            self.route_proof_generation = 0;
        }

        let result = node_lib::mobile_debt_settlement::retry_ambiguous_debt_settlement(
            std::path::Path::new(data_directory),
            &config.rpc_url,
        );
        if should_restart {
            let _ = self.start();
        } else {
            self.phase = Phase::Ready;
        }
        let status = result?;
        serde_json::to_string(&status)
            .map_err(|_| "The settlement retry result could not be encoded.".to_owned())
    }

    #[cfg(not(feature = "node-engine"))]
    pub fn retry_debt_settlement_json(&mut self) -> Result<String, String> {
        Err("The native MASQ accounting core is unavailable.".to_owned())
    }

    #[cfg(feature = "node-engine")]
    pub fn debt_settlement_status_json(&self) -> Result<String, String> {
        let config = self
            .config
            .as_ref()
            .ok_or_else(|| "Configure the MASQ network profile first.".to_owned())?;
        let status = node_lib::mobile_debt_settlement::refresh_debt_settlement_status(
            self.data_directory()?,
            &config.rpc_url,
        )?;
        serde_json::to_string(&status)
            .map_err(|_| "The settlement status could not be encoded.".to_owned())
    }

    #[cfg(not(feature = "node-engine"))]
    pub fn debt_settlement_status_json(&self) -> Result<String, String> {
        Err("The native MASQ accounting core is unavailable.".to_owned())
    }

    #[cfg(feature = "node-engine")]
    fn data_directory(&self) -> Result<&std::path::Path, String> {
        self.config
            .as_ref()
            .and_then(|config| config.data_directory.as_deref())
            .map(std::path::Path::new)
            .ok_or_else(|| "The protected MASQ data directory is unavailable.".to_owned())
    }

    pub fn status_json(&mut self) -> String {
        self.refresh_engine_status();
        self.status_json_without_refresh()
    }

    fn fail<T>(&mut self, message: &str) -> Result<T, String> {
        self.phase = Phase::Error;
        self.proxy_enabled = false;
        self.last_error = Some(message.to_owned());
        Err(message.to_owned())
    }

    pub fn record_error(&mut self, message: String) {
        self.phase = Phase::Error;
        self.proxy_enabled = false;
        self.last_error = Some(message);
    }

    fn refresh_engine_status(&mut self) {
        #[cfg(feature = "node-engine")]
        {
            let Some(engine) = self.engine.as_ref() else {
                return;
            };
            let snapshot = engine.snapshot();
            self.apply_engine_snapshot(&snapshot);
            if !snapshot.started && snapshot.last_exit_code.is_some() {
                self.engine
                    .as_mut()
                    .expect("engine presence checked above")
                    .reap_if_finished();
            }
        }
    }

    #[cfg(feature = "node-engine")]
    fn apply_engine_snapshot(&mut self, snapshot: &crate::engine::EngineSnapshot) {
        let preserve_explicit_error = matches!(self.phase, Phase::Error | Phase::Blocked);
        self.available_exit_countries = snapshot.available_exit_countries.clone();
        self.proxy_port = snapshot.proxy_port;
        self.bytes_up = snapshot.bytes_up;
        self.bytes_down = snapshot.bytes_down;
        self.route_stage = snapshot.route_stage.min(2);
        self.route_proof_generation = snapshot.route_proof_generation;
        self.connected_neighbors = usize::from(self.route_stage >= 1);
        self.route_hops = if self.route_stage >= 2 {
            snapshot.route_hops
        } else {
            0
        };
        let paused = matches!(self.phase, Phase::Paused);
        // Stage one proves only an entry-neighbor handshake. Browser and system traffic become
        // ready only after RouteFound (stage two), which represents correlated response bytes
        // received through a MASQ exit route. A route-proof expiry therefore degrades the
        // public state back to Connecting and immediately disables browser proxying.
        if snapshot.started && self.route_stage >= 2 {
            if !paused && !preserve_explicit_error {
                self.phase = Phase::Connected;
            }
            if !preserve_explicit_error {
                self.last_error = None;
            }
        } else if snapshot.started && self.route_stage == 1 {
            self.proxy_enabled = false;
            if !paused && !preserve_explicit_error {
                self.phase = Phase::Connecting;
                self.last_error = None;
            }
        } else if snapshot.started {
            self.proxy_enabled = false;
            if !paused && !preserve_explicit_error {
                self.phase = if snapshot.stop_requested {
                    Phase::Stopping
                } else {
                    Phase::Connecting
                };
                self.last_error = connection_timeout_message(snapshot);
            }
        } else if let Some(exit_code) = snapshot.last_exit_code {
            self.proxy_enabled = false;
            if snapshot.stop_requested && exit_code == 0 {
                self.phase = Phase::Ready;
                self.proxy_port = None;
                self.route_stage = 0;
                self.route_proof_generation = 0;
            } else {
                self.phase = Phase::Error;
                self.last_error = snapshot
                        .last_connection_error
                        .clone()
                        .filter(|error| error.starts_with("E_CORE_PANIC_LOCATION:"))
                        .or_else(|| {
                            Some(format!(
                                "The embedded MASQ Node stopped with code {exit_code}. Check the Node log."
                            ))
                        });
            }
        }
    }
}

#[cfg(feature = "node-engine")]
fn connection_timeout_message(snapshot: &crate::engine::EngineSnapshot) -> Option<String> {
    let (milestone, milestone_age, attempt_age) =
        node_lib::mobile_runtime::entry_handshake_progress();
    connection_timeout_message_for_milestone(snapshot, milestone, milestone_age, attempt_age)
}

#[cfg(feature = "node-engine")]
const ENTRY_PRE_DEBUT_IDLE_TIMEOUT: Duration = Duration::from_secs(18);
#[cfg(feature = "node-engine")]
const ENTRY_UNACCEPTED_DEBUT_IDLE_TIMEOUT: Duration = Duration::from_secs(8);
#[cfg(feature = "node-engine")]
const ENTRY_ACCEPTED_GOSSIP_PROMOTION_TIMEOUT: Duration = Duration::from_secs(26);
#[cfg(feature = "node-engine")]
const ENTRY_ATTEMPT_HARD_TIMEOUT: Duration = Duration::from_secs(45);

#[cfg(feature = "node-engine")]
fn connection_timeout_message_for_milestone(
    snapshot: &crate::engine::EngineSnapshot,
    milestone: node_lib::mobile_runtime::EntryHandshakeMilestone,
    milestone_age: std::time::Duration,
    attempt_age: std::time::Duration,
) -> Option<String> {
    use node_lib::mobile_runtime::EntryHandshakeMilestone;

    // Neighborhood publishes these only after every parallel entry attempt is
    // terminal, so it is safe to rotate the pair immediately.
    if let Some(error) = snapshot.last_connection_error.as_deref() {
        if error.starts_with("E_ENTRY_GOSSIP_PASS_LOOP:") {
            return Some(
                "E_ENTRY_GOSSIP_PASS_LOOP: The entry-node handshake encountered a pass loop."
                    .to_owned(),
            );
        }
        if error.starts_with("E_ENTRY_NO_PROGRESS:") {
            return Some(
                "E_ENTRY_NO_PROGRESS: All selected entry peers exhausted the initial handshake."
                    .to_owned(),
            );
        }
    }

    let inactivity_timeout = match milestone {
        EntryHandshakeMilestone::None | EntryHandshakeMilestone::TcpConnected => {
            ENTRY_PRE_DEBUT_IDLE_TIMEOUT
        }
        // Once Debut left the device, a compatible peer should begin its small
        // gossip response promptly. Every positive clandestine read refreshes
        // milestone_age, so a genuinely slow but progressing frame keeps its
        // full idle budget while a silent or invalid peer rotates quickly.
        EntryHandshakeMilestone::DebutBytesWritten
        | EntryHandshakeMilestone::InboundBytesReceived => ENTRY_UNACCEPTED_DEBUT_IDLE_TIMEOUT,
        // Validated gossip is stronger evidence than raw bytes. Keep the wider
        // actor/topology promotion window after acceptance.
        EntryHandshakeMilestone::GossipAccepted => ENTRY_ACCEPTED_GOSSIP_PROMOTION_TIMEOUT,
    };
    if milestone_age < inactivity_timeout && attempt_age < ENTRY_ATTEMPT_HARD_TIMEOUT {
        return None;
    }

    let message = match milestone {
        EntryHandshakeMilestone::None => {
            "E_ENTRY_TCP_FAILED: MASQ could not establish an entry-node TCP transport."
        }
        EntryHandshakeMilestone::TcpConnected => {
            "E_ENTRY_DEBUT_NOT_WRITTEN: MASQ could not confirm writing the entry handshake."
        }
        EntryHandshakeMilestone::DebutBytesWritten => {
            "E_ENTRY_NO_INBOUND_BYTES: MASQ wrote the entry handshake but received no reply bytes."
        }
        EntryHandshakeMilestone::InboundBytesReceived => {
            "E_ENTRY_INBOUND_NOT_ACCEPTED: MASQ received entry reply bytes, but they were not accepted as valid gossip."
        }
        EntryHandshakeMilestone::GossipAccepted => {
            "E_ENTRY_GOSSIP_NOT_PROMOTED: MASQ accepted entry gossip, but the connection did not advance to a usable neighbor."
        }
    };
    Some(message.to_owned())
}

fn engine_available() -> bool {
    cfg!(feature = "node-engine")
}

fn entry_retry_requires_fresh_identity(last_error: Option<&str>) -> bool {
    let code = last_error.map(|error| error.split_once(':').map_or(error, |(code, _)| code).trim());
    matches!(
        code,
        Some(
            "E_ENTRY_TCP_WAITING_GOSSIP"
                | "E_ENTRY_GOSSIP_TIMEOUT"
                | "E_ENTRY_GOSSIP_PASS_LOOP"
                | "E_ENTRY_NO_PROGRESS"
                | "E_ENTRY_NO_INBOUND_BYTES"
                | "E_ENTRY_INBOUND_NOT_ACCEPTED"
                | "E_ENTRY_GOSSIP_NOT_PROMOTED"
        )
    )
}

fn entry_retry_requires_fresh_runtime(
    phase: Phase,
    entry_nodes_changed: bool,
    last_error: Option<&str>,
) -> bool {
    entry_retry_requires_fresh_identity(last_error)
        || (entry_nodes_changed && matches!(phase, Phase::Connecting) && last_error.is_none())
}

#[cfg(feature = "node-engine")]
fn unix_seconds(time: std::time::SystemTime) -> Result<u64, String> {
    time.duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| "The settlement quote contains an invalid timestamp.".to_owned())
}

#[cfg(feature = "node-engine")]
fn parse_wei_limit(value: &str, label: &str) -> Result<u128, String> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(format!("The reviewed {label} is invalid."));
    }
    value
        .parse::<u128>()
        .map_err(|_| format!("The reviewed {label} is outside the supported range."))
}

#[cfg(feature = "node-engine")]
fn constant_time_text_eq(left: &str, right: &str) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.as_bytes()
        .iter()
        .zip(right.as_bytes())
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

#[cfg(feature = "node-engine")]
// A route that cannot complete CONNECT, TLS and a remote HTTP response within
// this fixed budget is not healthy enough for interactive mobile browsing.
const ROUTE_PROBE_TIMEOUT_SECONDS: u64 = 12;
#[cfg(feature = "node-engine")]
const ROUTE_PROBE_RETRY_DELAY: Duration = Duration::from_millis(400);

#[cfg(feature = "node-engine")]
struct DeadlineStream {
    stream: TcpStream,
    deadline: Instant,
}

#[cfg(feature = "node-engine")]
impl DeadlineStream {
    fn new(stream: TcpStream, deadline: Instant) -> Self {
        Self { stream, deadline }
    }

    fn prepare_read(&self) -> io::Result<()> {
        apply_probe_deadline(self.deadline, |timeout| {
            self.stream.set_read_timeout(timeout)
        })
    }

    fn prepare_write(&self) -> io::Result<()> {
        apply_probe_deadline(self.deadline, |timeout| {
            self.stream.set_write_timeout(timeout)
        })
    }
}

#[cfg(feature = "node-engine")]
impl Read for DeadlineStream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.prepare_read()?;
        self.stream.read(buffer)
    }
}

#[cfg(feature = "node-engine")]
impl Write for DeadlineStream {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.prepare_write()?;
        self.stream.write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.prepare_write()?;
        self.stream.flush()
    }
}

#[cfg(feature = "node-engine")]
fn remaining_probe_time(deadline: Instant) -> io::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "the MASQ route test timed out"))
}

#[cfg(feature = "node-engine")]
fn apply_probe_deadline(
    deadline: Instant,
    set_timeout: impl FnOnce(Option<Duration>) -> io::Result<()>,
) -> io::Result<()> {
    set_timeout(Some(remaining_probe_time(deadline)?))
}

#[cfg(feature = "node-engine")]
fn read_bounded_http_header(
    reader: &mut impl std::io::Read,
    maximum_bytes: usize,
) -> Result<Vec<u8>, String> {
    let mut header = Vec::with_capacity(maximum_bytes.min(1024));
    let mut chunk = [0_u8; 512];
    while header.len() < maximum_bytes {
        let remaining = maximum_bytes - header.len();
        let read_limit = remaining.min(chunk.len());
        let length = reader
            .read(&mut chunk[..read_limit])
            .map_err(|error| error.to_string())?;
        if length == 0 {
            return Err("the connection closed before an HTTP header arrived".to_owned());
        }
        header.extend_from_slice(&chunk[..length]);
        if header.windows(4).any(|window| window == b"\r\n\r\n") {
            return Ok(header);
        }
    }
    Err("the HTTP header exceeded the safe probe limit".to_owned())
}

#[cfg(feature = "node-engine")]
fn http_status_code(response: &[u8]) -> Option<u16> {
    let first_line_end = response.windows(2).position(|window| window == b"\r\n")?;
    let first_line = std::str::from_utf8(&response[..first_line_end]).ok()?;
    let mut fields = first_line.split_ascii_whitespace();
    match fields.next()? {
        "HTTP/1.0" | "HTTP/1.1" => {}
        _ => return None,
    }
    let status = fields.next()?;
    if status.len() != 3 || !status.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    status.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn healthy_core_for_route_refresh() -> MobileCore {
        let mut core = configured_core();
        core.phase = Phase::Connected;
        core.engine_generation = 7;
        core.proxy_enabled = true;
        core.proxy_port = Some(44_443);
        core.connected_neighbors = 1;
        core.route_stage = 2;
        core.route_hops = 3;
        core.route_proof_generation = 11;
        core
    }

    #[test]
    fn scheduled_route_refresh_failure_preserves_healthy_state_and_redacts_probe_error() {
        let mut core = healthy_core_for_route_refresh();
        let ticket = core.begin_route_proof_refresh().unwrap();
        let json = core.complete_route_proof_refresh(
            ticket,
            Err("socket 203.0.113.7:443 failed for private.example wallet 0xfeed".to_owned()),
        );

        assert_eq!(core.phase, Phase::Connected);
        assert!(core.proxy_enabled);
        assert_eq!(core.proxy_port, Some(44_443));
        assert_eq!(core.connected_neighbors, 1);
        assert_eq!(core.route_stage, 2);
        assert_eq!(core.route_hops, 3);
        assert_eq!(core.route_proof_generation, 11);
        assert_eq!(core.last_error, None);

        let status: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(status["phase"], "connected");
        assert_eq!(status["proxyEnabled"], true);
        assert_eq!(status["routeStage"], 2);
        assert_eq!(status["routeProofRefresh"]["attempted"], true);
        assert_eq!(status["routeProofRefresh"]["succeeded"], false);
        assert_eq!(
            status["routeProofRefresh"]["errorCode"],
            "E_PRIVATE_ROUTE_REFRESH_FAILED"
        );
        assert!(!json.contains("203.0.113.7"));
        assert!(!json.contains("private.example"));
        assert!(!json.contains("0xfeed"));
    }

    #[test]
    fn scheduled_route_refresh_failure_imports_engine_demotion_before_publishing_status() {
        let mut core = healthy_core_for_route_refresh();
        let ticket = core.begin_route_proof_refresh().unwrap();

        let json = core.complete_route_proof_refresh_with_hooks(
            ticket,
            Err("bounded route probe failed".to_owned()),
            |core| {
                core.phase = Phase::Connecting;
                core.proxy_enabled = false;
                core.connected_neighbors = 1;
                core.route_stage = 1;
                core.route_hops = 0;
            },
            |_| panic!("a failed probe must not report a successful route"),
        );

        let status: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(status["phase"], "connecting");
        assert_eq!(status["proxyEnabled"], false);
        assert_eq!(status["routeStage"], 1);
        assert_eq!(status["routeProofRefresh"]["attempted"], false);
        assert_eq!(
            status["routeProofRefresh"]["errorCode"],
            "E_PRIVATE_ROUTE_REFRESH_NOT_READY"
        );
    }

    #[test]
    fn scheduled_route_refresh_reports_stage_only_for_the_current_ticket() {
        use std::cell::Cell;

        let mut core = healthy_core_for_route_refresh();
        let ticket = core.begin_route_proof_refresh().unwrap();
        let report_calls = Cell::new(0);

        let json = core.complete_route_proof_refresh_with(ticket, Ok(()), |_| {
            report_calls.set(report_calls.get() + 1);
        });

        assert_eq!(report_calls.get(), 1);
        let status: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(status["routeProofRefresh"]["succeeded"], true);
        assert_eq!(
            status["routeProofRefresh"]["errorCode"],
            serde_json::Value::Null
        );
    }

    #[test]
    fn stale_route_refresh_success_never_invokes_the_runtime_report_callback() {
        use std::cell::Cell;

        let mut core = healthy_core_for_route_refresh();
        let ticket = core.begin_route_proof_refresh().unwrap();
        core.engine_generation += 1;
        let report_calls = Cell::new(0);

        let json = core.complete_route_proof_refresh_with(ticket, Ok(()), |_| {
            report_calls.set(report_calls.get() + 1);
        });

        assert_eq!(report_calls.get(), 0);
        let status: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(status["engineGeneration"], 8);
        assert_eq!(status["routeProofRefresh"]["attempted"], false);
        assert_eq!(status["routeProofRefresh"]["succeeded"], false);
    }

    #[test]
    fn route_refresh_revalidates_health_after_the_runtime_report_callback() {
        let mut core = healthy_core_for_route_refresh();
        let ticket = core.begin_route_proof_refresh().unwrap();

        let json = core.complete_route_proof_refresh_with(ticket, Ok(()), |core| {
            core.route_stage = 1;
            core.proxy_enabled = false;
        });

        let status: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(status["routeStage"], 1);
        assert_eq!(status["proxyEnabled"], false);
        assert_eq!(status["routeProofRefresh"]["attempted"], false);
        assert_eq!(status["routeProofRefresh"]["succeeded"], false);
    }

    #[cfg(feature = "node-engine")]
    #[test]
    fn explicit_preflight_failure_is_transient_and_privacy_safe() {
        let mut core = configured_core();
        core.phase = Phase::Connecting;
        core.engine_generation = 9;
        core.proxy_port = Some(44_443);
        core.connected_neighbors = 1;
        core.route_stage = 1;

        let json = core.preflight_proxy_status_json_with(|_| {
            Err("peer 203.0.113.8 failed while loading private.example".to_owned())
        });
        let response: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(response["phase"], "error");
        assert_eq!(response["proxyEnabled"], false);
        assert_eq!(
            response["lastError"],
            "E_PRIVATE_ROUTE_FAILED: MASQ could not yet prove an end-to-end private exit route."
        );
        assert!(!json.contains("203.0.113.8"));
        assert!(!json.contains("private.example"));
        assert_eq!(core.phase, Phase::Connecting);
        assert_eq!(core.connected_neighbors, 1);
        assert_eq!(core.route_stage, 1);
        assert_eq!(core.last_error, None);
    }

    #[test]
    fn only_post_debut_entry_failures_require_a_fresh_identity() {
        for code in [
            "E_ENTRY_TCP_WAITING_GOSSIP",
            "E_ENTRY_GOSSIP_TIMEOUT",
            "E_ENTRY_GOSSIP_PASS_LOOP",
            "E_ENTRY_NO_PROGRESS",
            "E_ENTRY_NO_INBOUND_BYTES",
            "E_ENTRY_INBOUND_NOT_ACCEPTED",
            "E_ENTRY_GOSSIP_NOT_PROMOTED",
        ] {
            assert!(entry_retry_requires_fresh_identity(Some(&format!(
                "{code}: safe diagnostic"
            ))));
        }
        for error in [
            None,
            Some("E_ENTRY_TCP_FAILED: no TCP transport"),
            Some("E_ENTRY_DEBUT_NOT_WRITTEN: no Debut bytes"),
            Some("E_PRIVATE_ROUTE_FAILED: exit proof failed"),
            Some("unstructured error"),
        ] {
            assert!(!entry_retry_requires_fresh_identity(error));
        }

        assert!(entry_retry_requires_fresh_runtime(
            Phase::Connecting,
            true,
            None,
        ));
        assert!(!entry_retry_requires_fresh_runtime(
            Phase::Connecting,
            false,
            None,
        ));
        assert!(!entry_retry_requires_fresh_runtime(
            Phase::Connecting,
            true,
            Some("E_ENTRY_TCP_FAILED: no TCP transport"),
        ));
    }

    #[cfg(feature = "node-engine")]
    #[test]
    fn configuring_new_entries_after_a_post_debut_failure_reaps_the_old_engine() {
        let mut core = configured_core();
        core.engine = Some(crate::engine::EngineHandle::finished_for_state_transition_test());
        core.engine_generation = 7;
        core.phase = Phase::Connecting;
        core.last_error = Some(
            "E_ENTRY_NO_INBOUND_BYTES: Debut was written but the peer stayed silent.".to_owned(),
        );

        core.configure(
            r#"{"chain":"base-mainnet","rpcUrl":"https://rpc.example","neighbors":["masq://base-mainnet:key2@example.net:4434"]}"#,
        )
        .unwrap();

        assert!(core.engine.is_none());
        assert_eq!(core.engine_generation, 7);
        assert_eq!(core.phase, Phase::Ready);
        assert_eq!(core.last_error, None);
        assert_eq!(
            core.config.as_ref().unwrap().neighbors,
            vec!["masq://base-mainnet:key2@example.net:4434"]
        );
        assert!(core.wallet.is_some());
    }

    #[cfg(feature = "node-engine")]
    #[test]
    fn configuring_new_entries_before_debut_keeps_the_live_engine_for_in_place_retry() {
        let mut core = configured_core();
        core.engine = Some(crate::engine::EngineHandle::finished_for_state_transition_test());
        core.engine_generation = 7;
        core.phase = Phase::Connecting;
        core.last_error = Some(
            "E_ENTRY_DEBUT_NOT_WRITTEN: TCP connected but no Debut bytes left the device."
                .to_owned(),
        );

        core.configure(
            r#"{"chain":"base-mainnet","rpcUrl":"https://rpc.example","neighbors":["masq://base-mainnet:key2@example.net:4434"]}"#,
        )
        .unwrap();

        assert!(core.engine.is_some());
        assert_eq!(core.engine_generation, 7);
        assert_eq!(core.phase, Phase::Ready);
        assert_eq!(core.last_error, None);
        assert!(core.wallet.is_some());
    }

    #[test]
    fn scheduled_route_refresh_does_not_probe_an_unhealthy_route() {
        let mut core = healthy_core_for_route_refresh();
        core.route_stage = 1;

        assert_eq!(core.begin_route_proof_refresh(), None);
        assert_eq!(core.phase, Phase::Connected);
        assert_eq!(core.route_stage, 1);
    }

    #[cfg(not(feature = "node-engine"))]
    #[test]
    fn scheduled_route_refresh_reports_a_fixed_unavailable_code_without_an_engine() {
        let mut core = MobileCore::default();

        let json = core.route_proof_refresh_unavailable_json();
        let status: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(status["phase"], "unconfigured");
        assert_eq!(status["routeProofRefresh"]["attempted"], false);
        assert_eq!(status["routeProofRefresh"]["succeeded"], false);
        assert_eq!(
            status["routeProofRefresh"]["errorCode"],
            "E_PRIVATE_ROUTE_REFRESH_UNAVAILABLE"
        );
    }

    #[cfg(feature = "node-engine")]
    #[test]
    fn route_probe_deadline_expires_before_attempting_socket_configuration() {
        let mut setter_called = false;
        let expired = Instant::now()
            .checked_sub(Duration::from_millis(1))
            .expect("a one-millisecond-old deadline is representable");
        let result = apply_probe_deadline(expired, |_| {
            setter_called = true;
            Ok(())
        });

        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
        assert!(!setter_called);
    }

    #[cfg(feature = "node-engine")]
    #[test]
    fn route_probe_budget_remains_fail_fast_for_interactive_browsing() {
        assert_eq!(ROUTE_PROBE_TIMEOUT_SECONDS, 12);
    }

    #[cfg(feature = "node-engine")]
    #[test]
    fn route_probe_propagates_socket_timeout_configuration_failure() {
        let result = apply_probe_deadline(Instant::now() + Duration::from_secs(1), |_| {
            Err(io::Error::new(io::ErrorKind::PermissionDenied, "denied"))
        });

        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
    }

    #[cfg(feature = "node-engine")]
    #[test]
    fn route_probe_requires_a_complete_bounded_http_response_header() {
        let mut complete = &b"HTTP/1.1 204 No Content\r\nX-Test: yes\r\n\r\n"[..];
        let header = read_bounded_http_header(&mut complete, 1024).unwrap();
        assert_eq!(http_status_code(&header), Some(204));

        let mut incomplete = &b"HTTP/1.1 200 OK\r\nX-Test: no"[..];
        assert!(read_bounded_http_header(&mut incomplete, 1024).is_err());

        let oversized_bytes = vec![b'a'; 64];
        let mut oversized = oversized_bytes.as_slice();
        assert!(read_bounded_http_header(&mut oversized, 32).is_err());
        assert_eq!(http_status_code(b"not-http\r\n\r\n"), None);
        assert_eq!(http_status_code(b"HTTP/2 200\r\n\r\n"), None);
    }

    #[cfg(feature = "node-engine")]
    fn engine_snapshot(error: Option<&str>, seconds: u64) -> crate::engine::EngineSnapshot {
        engine_snapshot_with_route(0, 1, error, seconds)
    }

    #[cfg(feature = "node-engine")]
    fn engine_snapshot_with_route(
        route_stage: u8,
        route_hops: usize,
        error: Option<&str>,
        seconds: u64,
    ) -> crate::engine::EngineSnapshot {
        crate::engine::EngineSnapshot {
            started: true,
            stop_requested: false,
            proxy_port: Some(44_443),
            route_stage,
            route_hops,
            route_proof_generation: 0,
            bytes_up: 0,
            bytes_down: 0,
            last_exit_code: None,
            running_for: std::time::Duration::from_secs(seconds),
            last_connection_error: error.map(str::to_owned),
            available_exit_countries: vec![],
        }
    }

    fn configured_core() -> MobileCore {
        let mut core = MobileCore::default();
        core.configure(
            r#"{"chain":"base-mainnet","rpcUrl":"https://rpc.example","neighbors":["masq://base-mainnet:key@example.org:4433"]}"#,
        )
        .unwrap();
        core.import_wallet("0000000000000000000000000000000000000000000000000000000000000001")
            .unwrap();
        core
    }

    #[cfg(not(feature = "node-engine"))]
    #[test]
    fn default_build_blocks_instead_of_bypassing_masq() {
        let mut core = configured_core();
        core.start().unwrap();
        let status: serde_json::Value = serde_json::from_str(&core.status_json()).unwrap();
        assert_eq!(status["phase"], "blocked");
        assert_eq!(status["engineAvailable"], false);
        assert_eq!(status["proxyEnabled"], false);
        assert!(core.set_proxy_enabled(true).is_err());
    }

    #[test]
    fn reset_forgets_the_wallet_and_network_profile() {
        let mut core = configured_core();
        core.reset();

        let status: serde_json::Value = serde_json::from_str(&core.status_json()).unwrap();
        assert_eq!(status["phase"], "unconfigured");
        assert_eq!(status["chain"], serde_json::Value::Null);
        assert_eq!(status["walletAddress"], serde_json::Value::Null);
    }

    #[test]
    fn network_reset_keeps_the_wallet() {
        let mut core = configured_core();
        core.reset_network_profile();

        let status: serde_json::Value = serde_json::from_str(&core.status_json()).unwrap();
        assert_eq!(status["phase"], "unconfigured");
        assert_eq!(status["chain"], serde_json::Value::Null);
        assert!(status["walletAddress"].as_str().is_some());
    }

    #[test]
    fn wallet_removal_keeps_the_network_profile() {
        let mut core = configured_core();
        core.remove_wallet();

        let status: serde_json::Value = serde_json::from_str(&core.status_json()).unwrap();
        assert_eq!(status["phase"], "unconfigured");
        assert_eq!(status["chain"], "base-mainnet");
        assert_eq!(status["walletAddress"], serde_json::Value::Null);
    }

    #[test]
    fn shutdown_preserves_the_wallet_and_profile_in_a_disconnected_state() {
        let mut core = configured_core();
        core.shutdown();

        let status: serde_json::Value = serde_json::from_str(&core.status_json()).unwrap();
        assert_eq!(status["phase"], "ready");
        assert_eq!(status["connectedNeighbors"], 0);
        assert_eq!(status["proxyEnabled"], false);
        assert_eq!(status["proxyPort"], serde_json::Value::Null);
        assert_eq!(status["chain"], "base-mainnet");
        assert!(status["walletAddress"].as_str().is_some());
    }

    #[test]
    fn status_reports_route_and_exit_preferences() {
        let mut core = MobileCore::default();
        core.configure(
            r#"{"chain":"base-mainnet","rpcUrl":"https://rpc.example","neighbors":["masq://base-mainnet:key@example.org:4433"],"minHops":3,"exitCountry":"BE","exitCountryFallback":false}"#,
        )
        .unwrap();

        let status: serde_json::Value = serde_json::from_str(&core.status_json()).unwrap();
        assert_eq!(status["minHops"], 3);
        assert_eq!(status["exitCountry"], "BE");
        assert_eq!(status["exitCountryFallback"], false);
    }

    #[test]
    fn route_length_can_change_without_forgetting_the_profile_or_wallet() {
        let mut core = configured_core();
        core.update_min_hops(4).unwrap();

        let status: serde_json::Value = serde_json::from_str(&core.status_json()).unwrap();
        assert_eq!(status["phase"], "ready");
        assert_eq!(status["minHops"], 4);
        assert_eq!(status["chain"], "base-mainnet");
        assert!(status["walletAddress"].as_str().is_some());
    }

    #[test]
    fn route_length_rejects_values_outside_the_masq_range() {
        let mut core = configured_core();
        assert!(core.update_min_hops(0).is_err());
        assert!(core.update_min_hops(7).is_err());
    }

    #[cfg(feature = "node-engine")]
    #[test]
    fn node_engine_build_reports_the_real_engine_as_available() {
        let mut core = configured_core();
        let status: serde_json::Value = serde_json::from_str(&core.status_json()).unwrap();
        assert_eq!(status["phase"], "ready");
        assert_eq!(status["engineAvailable"], true);
        assert_eq!(status["proxyEnabled"], false);
    }

    #[cfg(feature = "node-engine")]
    #[test]
    fn entry_neighbor_remains_connecting_until_an_exit_route_is_proven() {
        let mut core = configured_core();
        core.phase = Phase::Connecting;
        core.proxy_enabled = true;

        core.apply_engine_snapshot(&engine_snapshot_with_route(1, 4, None, 5));

        let status: serde_json::Value = serde_json::from_str(&core.status_json()).unwrap();
        assert_eq!(status["phase"], "connecting");
        assert_eq!(status["connectedNeighbors"], 1);
        assert_eq!(status["routeStage"], 1);
        assert_eq!(status["routeHops"], 0);
        assert_eq!(status["proxyEnabled"], false);
    }

    #[cfg(feature = "node-engine")]
    #[test]
    fn correlated_exit_response_marks_the_private_route_connected() {
        let mut core = configured_core();
        core.phase = Phase::Connecting;

        core.apply_engine_snapshot(&engine_snapshot_with_route(2, 4, None, 5));

        let status: serde_json::Value = serde_json::from_str(&core.status_json()).unwrap();
        assert_eq!(status["phase"], "connected");
        assert_eq!(status["connectedNeighbors"], 1);
        assert_eq!(status["routeStage"], 2);
        assert_eq!(status["routeHops"], 4);
        assert_eq!(status["proxyPort"], 44_443);
    }

    #[cfg(feature = "node-engine")]
    #[test]
    fn route_proof_degradation_revokes_connected_and_proxy_states() {
        let mut core = configured_core();
        core.phase = Phase::Connecting;
        core.apply_engine_snapshot(&engine_snapshot_with_route(2, 3, None, 5));
        core.proxy_enabled = true;

        core.apply_engine_snapshot(&engine_snapshot_with_route(1, 3, None, 6));

        assert_eq!(core.phase, Phase::Connecting);
        assert_eq!(core.connected_neighbors, 1);
        assert_eq!(core.route_stage, 1);
        assert_eq!(core.route_hops, 0);
        assert!(!core.proxy_enabled);
    }

    #[cfg(feature = "node-engine")]
    #[test]
    fn entry_progress_does_not_hide_an_explicit_route_probe_error() {
        let mut core = configured_core();
        core.phase = Phase::Connecting;
        core.record_error("E_PRIVATE_ROUTE_FAILED: probe failed".to_owned());

        core.apply_engine_snapshot(&engine_snapshot_with_route(1, 3, None, 6));

        assert_eq!(core.phase, Phase::Error);
        assert_eq!(core.route_stage, 1);
        assert_eq!(
            core.last_error.as_deref(),
            Some("E_PRIVATE_ROUTE_FAILED: probe failed")
        );
    }

    #[cfg(feature = "node-engine")]
    #[test]
    fn entry_node_timeout_is_short_only_while_post_debut_gossip_is_unaccepted() {
        use node_lib::mobile_runtime::EntryHandshakeMilestone;

        let transport_snapshot = engine_snapshot(None, 17);
        assert_eq!(
            connection_timeout_message_for_milestone(
                &transport_snapshot,
                EntryHandshakeMilestone::TcpConnected,
                std::time::Duration::from_secs(17),
                std::time::Duration::from_secs(17),
            ),
            None
        );
        let progressing_inbound_snapshot = engine_snapshot(None, 35);
        assert_eq!(
            connection_timeout_message_for_milestone(
                &progressing_inbound_snapshot,
                EntryHandshakeMilestone::InboundBytesReceived,
                ENTRY_UNACCEPTED_DEBUT_IDLE_TIMEOUT - std::time::Duration::from_millis(1),
                std::time::Duration::from_secs(35),
            ),
            None,
            "recent inbound activity must protect a slow but progressing peer"
        );
        let progressed_snapshot = engine_snapshot(None, 25);
        assert_eq!(
            connection_timeout_message_for_milestone(
                &progressed_snapshot,
                EntryHandshakeMilestone::GossipAccepted,
                std::time::Duration::from_secs(25),
                std::time::Duration::from_secs(25),
            ),
            None
        );
    }

    #[cfg(feature = "node-engine")]
    #[test]
    fn silent_or_unaccepted_post_debut_gossip_expires_after_eight_idle_seconds() {
        use node_lib::mobile_runtime::EntryHandshakeMilestone;

        let snapshot = engine_snapshot(None, 8);
        for (milestone, expected_code) in [
            (
                EntryHandshakeMilestone::DebutBytesWritten,
                "E_ENTRY_NO_INBOUND_BYTES:",
            ),
            (
                EntryHandshakeMilestone::InboundBytesReceived,
                "E_ENTRY_INBOUND_NOT_ACCEPTED:",
            ),
        ] {
            let diagnostic = connection_timeout_message_for_milestone(
                &snapshot,
                milestone,
                ENTRY_UNACCEPTED_DEBUT_IDLE_TIMEOUT,
                ENTRY_UNACCEPTED_DEBUT_IDLE_TIMEOUT,
            )
            .expect("post-Debut inactivity diagnostic expected");
            assert!(
                diagnostic.starts_with(expected_code),
                "expected {expected_code}, got {diagnostic}"
            );
        }
    }

    #[cfg(feature = "node-engine")]
    #[test]
    fn entry_timeout_codes_identify_the_last_privacy_safe_handshake_milestone() {
        use node_lib::mobile_runtime::EntryHandshakeMilestone;

        let transport_snapshot = engine_snapshot(None, 18);
        let unaccepted_debut_snapshot = engine_snapshot(None, 8);
        let progressed_snapshot = engine_snapshot(None, 26);
        let cases = [
            (EntryHandshakeMilestone::None, "E_ENTRY_TCP_FAILED:"),
            (
                EntryHandshakeMilestone::TcpConnected,
                "E_ENTRY_DEBUT_NOT_WRITTEN:",
            ),
            (
                EntryHandshakeMilestone::DebutBytesWritten,
                "E_ENTRY_NO_INBOUND_BYTES:",
            ),
            (
                EntryHandshakeMilestone::InboundBytesReceived,
                "E_ENTRY_INBOUND_NOT_ACCEPTED:",
            ),
            (
                EntryHandshakeMilestone::GossipAccepted,
                "E_ENTRY_GOSSIP_NOT_PROMOTED:",
            ),
        ];

        for (milestone, expected_code) in cases {
            let snapshot = match milestone {
                EntryHandshakeMilestone::DebutBytesWritten
                | EntryHandshakeMilestone::InboundBytesReceived => &unaccepted_debut_snapshot,
                EntryHandshakeMilestone::GossipAccepted => &progressed_snapshot,
                EntryHandshakeMilestone::None | EntryHandshakeMilestone::TcpConnected => {
                    &transport_snapshot
                }
            };
            let milestone_age = match milestone {
                EntryHandshakeMilestone::DebutBytesWritten
                | EntryHandshakeMilestone::InboundBytesReceived => {
                    ENTRY_UNACCEPTED_DEBUT_IDLE_TIMEOUT
                }
                EntryHandshakeMilestone::GossipAccepted => ENTRY_ACCEPTED_GOSSIP_PROMOTION_TIMEOUT,
                EntryHandshakeMilestone::None | EntryHandshakeMilestone::TcpConnected => {
                    ENTRY_PRE_DEBUT_IDLE_TIMEOUT
                }
            };
            let message = connection_timeout_message_for_milestone(
                snapshot,
                milestone,
                milestone_age,
                milestone_age,
            )
            .expect("diagnostic expected");
            assert!(
                message.starts_with(expected_code),
                "expected {expected_code}, got {message}"
            );
        }
    }

    #[cfg(feature = "node-engine")]
    #[test]
    fn aggregate_pass_loop_code_bypasses_the_fallback_watchdog() {
        use node_lib::mobile_runtime::EntryHandshakeMilestone;

        let snapshot = engine_snapshot(
            Some("E_ENTRY_GOSSIP_PASS_LOOP: internal fixed diagnostic"),
            1,
        );

        assert_eq!(
            connection_timeout_message_for_milestone(
                &snapshot,
                EntryHandshakeMilestone::InboundBytesReceived,
                std::time::Duration::ZERO,
                std::time::Duration::ZERO,
            ),
            Some(
                "E_ENTRY_GOSSIP_PASS_LOOP: The entry-node handshake encountered a pass loop."
                    .to_owned()
            )
        );
    }

    #[cfg(feature = "node-engine")]
    #[test]
    fn terminal_entry_progress_code_bypasses_the_fallback_watchdog() {
        use node_lib::mobile_runtime::EntryHandshakeMilestone;

        let snapshot = engine_snapshot(Some("E_ENTRY_NO_PROGRESS: fixed internal diagnostic"), 1);

        assert_eq!(
            connection_timeout_message_for_milestone(
                &snapshot,
                EntryHandshakeMilestone::DebutBytesWritten,
                std::time::Duration::ZERO,
                std::time::Duration::ZERO,
            ),
            Some(
                "E_ENTRY_NO_PROGRESS: All selected entry peers exhausted the initial handshake."
                    .to_owned()
            )
        );
    }

    #[cfg(feature = "node-engine")]
    #[test]
    fn entry_hard_cap_is_scoped_to_the_current_attempt_not_engine_uptime() {
        use node_lib::mobile_runtime::EntryHandshakeMilestone;

        let long_running_engine = engine_snapshot(None, 3_600);
        assert_eq!(
            connection_timeout_message_for_milestone(
                &long_running_engine,
                EntryHandshakeMilestone::DebutBytesWritten,
                std::time::Duration::from_secs(1),
                std::time::Duration::from_secs(1),
            ),
            None
        );
        assert!(connection_timeout_message_for_milestone(
            &long_running_engine,
            EntryHandshakeMilestone::DebutBytesWritten,
            std::time::Duration::ZERO,
            std::time::Duration::from_secs(45),
        )
        .expect("current-attempt hard-cap diagnostic expected")
        .starts_with("E_ENTRY_NO_INBOUND_BYTES:"));
    }
}
