use serde::Serialize;

use crate::config::{Chain, MobileConfig};
#[cfg(feature = "node-engine")]
use crate::engine::EngineHandle;
use crate::wallet::WalletMaterial;

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

pub struct MobileCore {
    config: Option<MobileConfig>,
    wallet: Option<WalletMaterial>,
    phase: Phase,
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
}

impl Default for MobileCore {
    fn default() -> Self {
        Self {
            config: None,
            wallet: None,
            phase: Phase::Unconfigured,
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
                engine.retry_connection(
                    &self
                        .config
                        .as_ref()
                        .expect("configuration checked above")
                        .neighbors,
                )?;
                self.phase = Phase::Connecting;
                self.proxy_enabled = false;
                self.connected_neighbors = 0;
                self.route_hops = 0;
                self.last_error = None;
                self.refresh_engine_status();
                return Ok(());
            }
            let engine = match EngineHandle::start(
                self.config.as_ref().expect("configuration checked above"),
                self.wallet.as_ref().expect("wallet checked above"),
            ) {
                Ok(engine) => engine,
                Err(error) => return self.fail(&error),
            };
            self.engine = Some(engine);
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
        use std::io::{Read, Write};
        use std::net::{Ipv4Addr, SocketAddrV4, TcpStream};
        use std::time::Duration;

        if !matches!(self.phase, Phase::Connected) {
            return self.fail("Connect a MASQ route before testing it.");
        }
        let port = self
            .proxy_port
            .ok_or_else(|| "The local MASQ proxy has no port.".to_owned())?;
        let timeout = Duration::from_secs(12);
        let mut stream = TcpStream::connect_timeout(
            &SocketAddrV4::new(Ipv4Addr::LOCALHOST, port).into(),
            timeout,
        )
        .map_err(|error| format!("The local MASQ proxy could not be reached: {error}"))?;
        stream.set_read_timeout(Some(timeout)).ok();
        stream.set_write_timeout(Some(timeout)).ok();
        stream
            .write_all(b"CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\nConnection: close\r\n\r\n")
            .map_err(|error| format!("The MASQ route test could not be sent: {error}"))?;
        let mut response = [0_u8; 1024];
        let length = stream
            .read(&mut response)
            .map_err(|error| format!("The MASQ exit route did not answer: {error}"))?;
        let header = String::from_utf8_lossy(&response[..length]);
        if header.starts_with("HTTP/1.1 200") || header.starts_with("HTTP/1.0 200") {
            node_lib::mobile_runtime::report_route_stage(2);
            self.refresh_engine_status();
            self.last_error = None;
            return Ok(());
        }
        Err("The MASQ exit route rejected the private browser preflight.".to_owned())
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

    pub fn status_json(&mut self) -> String {
        self.refresh_engine_status();
        let status = CoreStatus {
            phase: self.phase,
            engine_available: engine_available(),
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
                    self.last_error = Some(format!(
                        "The embedded MASQ Node stopped with code {exit_code}. Check the Node log."
                    ));
                }
            }
        }
    }
}

#[cfg(feature = "node-engine")]
fn connection_timeout_message(snapshot: &crate::engine::EngineSnapshot) -> Option<String> {
    if snapshot.running_for < std::time::Duration::from_secs(12) {
        return None;
    }
    match snapshot.last_connection_error.as_deref() {
        Some(error) if error.contains("Operation not permitted") => Some(
            "The operating system blocked the TCP connection to the MASQ entry nodes. Allow network access for MASQ in device settings, then try again."
                .to_owned(),
        ),
        Some(error) if error.contains("timed out") => Some(
            "The MASQ entry nodes did not answer in time. Open Node & wallet settings to select fresh nodes."
                .to_owned(),
        ),
        Some(error) => Some(format!(
            "The MASQ entry-node handshake failed: {error}. Open Node & wallet settings to select fresh nodes."
        )),
        None => Some(
            "MASQ is still waiting for an entry node. Check the device network connection or select fresh nodes."
                .to_owned(),
        ),
    }
}

fn engine_available() -> bool {
    cfg!(feature = "node-engine")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "node-engine")]
    fn engine_snapshot(error: Option<&str>, seconds: u64) -> crate::engine::EngineSnapshot {
        crate::engine::EngineSnapshot {
            started: true,
            stop_requested: false,
            proxy_port: Some(44_443),
            route_stage: 0,
            route_hops: 1,
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
        let snapshot = engine_snapshot(Some("Operation not permitted (os error 1)"), 11);
        assert_eq!(connection_timeout_message(&snapshot), None);
    }

    #[cfg(feature = "node-engine")]
    #[test]
    fn explains_operating_system_socket_denials_after_the_retry_window() {
        let snapshot = engine_snapshot(Some("Operation not permitted (os error 1)"), 12);
        assert!(connection_timeout_message(&snapshot)
            .expect("diagnostic expected")
            .contains("Allow network access"));
    }
}
