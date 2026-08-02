use serde::Serialize;

#[cfg(feature = "node-engine")]
use std::{
    io::{self, Read, Write},
    net::TcpStream,
    time::{Duration, Instant},
};

use crate::config::{Chain, MobileConfig};
#[cfg(feature = "node-engine")]
use crate::engine::{EngineHandle, RetryConnectionOutcome};
use crate::wallet::WalletMaterial;

#[cfg(feature = "node-engine")]
use node_lib::mobile_debt_settlement::PreparedDebtSettlement;

#[derive(Clone, Copy, Debug, Serialize)]
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
    min_hops: u8,
    exit_country: Option<&'a str>,
    exit_country_fallback: bool,
    available_exit_countries: &'a [String],
    bytes_up: u64,
    bytes_down: u64,
    last_error: Option<&'a str>,
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
    route_hops: usize,
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
            route_hops: 0,
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
            if !only_entry_nodes_changed {
                return self
                    .fail("Fully restart the app before changing the chain or blockchain RPC.");
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
                        self.route_hops = 0;
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
        self.route_hops = 0;
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
        self.route_hops = 0;
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
        self.route_hops = 0;
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
        self.route_hops = 0;
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

    #[cfg(feature = "node-engine")]
    pub fn preflight_proxy(&mut self) -> Result<(), String> {
        use std::net::{Ipv4Addr, SocketAddrV4};

        if !matches!(self.phase, Phase::Connected) {
            return self.fail("Connect a MASQ route before testing it.");
        }
        let port = self
            .proxy_port
            .ok_or_else(|| "The local MASQ proxy has no port.".to_owned())?;
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(ROUTE_PROBE_TIMEOUT_SECONDS))
            .ok_or_else(|| "The MASQ route-test deadline could not be created.".to_owned())?;
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

        node_lib::mobile_runtime::report_route_stage(2);
        self.refresh_engine_status();
        self.last_error = None;
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
            self.route_hops = 0;
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
            self.route_hops = 0;
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
        let status = CoreStatus {
            phase: self.phase,
            engine_available: engine_available(),
            engine_generation: self.engine_generation,
            proxy_enabled: self.proxy_enabled,
            proxy_port: self.proxy_port,
            chain: self.config.as_ref().map(|config| config.chain),
            wallet_address: self.wallet.as_ref().map(WalletMaterial::address),
            connected_neighbors: self.connected_neighbors,
            route_stage: if self.route_hops > 0 {
                2
            } else if self.connected_neighbors > 0 {
                1
            } else {
                0
            },
            route_hops: self.route_hops,
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
            let Some(engine) = self.engine.as_mut() else {
                return;
            };
            let snapshot = engine.snapshot();
            self.available_exit_countries = snapshot.available_exit_countries.clone();
            self.proxy_port = snapshot.proxy_port;
            self.bytes_up = snapshot.bytes_up;
            self.bytes_down = snapshot.bytes_down;
            let paused = matches!(self.phase, Phase::Paused);
            // A confirmed neighbor is enough to expose the fail-closed local proxy. The Node's
            // RouteFound stage requires correlated response traffic, which cannot exist until the
            // user opens the proxied browser for the first time.
            if snapshot.route_stage >= 1 {
                if !paused {
                    self.phase = Phase::Connected;
                }
                self.connected_neighbors = 1;
                self.route_hops = usize::from(snapshot.route_stage >= 2) * snapshot.route_hops;
                self.last_error = None;
            } else if snapshot.started {
                self.phase = if snapshot.stop_requested {
                    Phase::Stopping
                } else {
                    Phase::Connecting
                };
                self.connected_neighbors = 0;
                self.route_hops = 0;
                self.last_error = connection_timeout_message(&snapshot);
            } else if let Some(exit_code) = snapshot.last_exit_code {
                engine.reap_if_finished();
                if snapshot.stop_requested && exit_code == 0 {
                    self.phase = Phase::Ready;
                    self.proxy_port = None;
                } else {
                    self.phase = Phase::Error;
                    self.proxy_enabled = false;
                    self.last_error = snapshot
                        .last_connection_error
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
}

#[cfg(feature = "node-engine")]
fn connection_timeout_message(snapshot: &crate::engine::EngineSnapshot) -> Option<String> {
    connection_timeout_message_for_milestone(
        snapshot,
        node_lib::mobile_runtime::entry_handshake_milestone(),
    )
}

#[cfg(feature = "node-engine")]
fn connection_timeout_message_for_milestone(
    snapshot: &crate::engine::EngineSnapshot,
    milestone: node_lib::mobile_runtime::EntryHandshakeMilestone,
) -> Option<String> {
    use node_lib::mobile_runtime::EntryHandshakeMilestone;

    if snapshot.running_for < std::time::Duration::from_secs(32) {
        return None;
    }

    // A pass loop is a protocol-level terminal signal and is more useful than
    // the transport milestone that happened before it. It remains fixed text:
    // no descriptor, address, identity, payload, or OS error is surfaced.
    if snapshot
        .last_connection_error
        .as_deref()
        .map(|error| error.starts_with("E_ENTRY_GOSSIP_PASS_LOOP:"))
        .unwrap_or(false)
    {
        return Some(
            "E_ENTRY_GOSSIP_PASS_LOOP: The entry-node handshake encountered a pass loop."
                .to_owned(),
        );
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
const ROUTE_PROBE_TIMEOUT_SECONDS: u64 = 30;

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
        crate::engine::EngineSnapshot {
            started: true,
            stop_requested: false,
            proxy_port: Some(44_443),
            route_stage: 0,
            route_hops: 1,
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
    fn delays_entry_node_errors_during_the_initial_retry_window() {
        use node_lib::mobile_runtime::EntryHandshakeMilestone;

        let snapshot = engine_snapshot(None, 31);
        assert_eq!(
            connection_timeout_message_for_milestone(
                &snapshot,
                EntryHandshakeMilestone::GossipAccepted
            ),
            None
        );
    }

    #[cfg(feature = "node-engine")]
    #[test]
    fn entry_timeout_codes_identify_the_last_privacy_safe_handshake_milestone() {
        use node_lib::mobile_runtime::EntryHandshakeMilestone;

        let snapshot = engine_snapshot(None, 32);
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
            let message = connection_timeout_message_for_milestone(&snapshot, milestone)
                .expect("diagnostic expected");
            assert!(
                message.starts_with(expected_code),
                "expected {expected_code}, got {message}"
            );
        }
    }

    #[cfg(feature = "node-engine")]
    #[test]
    fn pass_loop_code_takes_priority_over_transport_milestones() {
        use node_lib::mobile_runtime::EntryHandshakeMilestone;

        let snapshot = engine_snapshot(
            Some("E_ENTRY_GOSSIP_PASS_LOOP: internal fixed diagnostic"),
            32,
        );

        assert_eq!(
            connection_timeout_message_for_milestone(
                &snapshot,
                EntryHandshakeMilestone::InboundBytesReceived
            ),
            Some(
                "E_ENTRY_GOSSIP_PASS_LOOP: The entry-node handshake encountered a pass loop."
                    .to_owned()
            )
        );
    }
}
