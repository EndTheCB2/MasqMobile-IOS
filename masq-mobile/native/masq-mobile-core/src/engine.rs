use std::fs;
use std::net::{Ipv4Addr, TcpListener};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use zeroize::Zeroize;

use crate::config::MobileConfig;
use crate::wallet::WalletMaterial;

pub struct EngineHandle {
    thread: Option<JoinHandle<i32>>,
    proxy_port: u16,
    min_hops: u8,
    started_at: Instant,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetryConnectionOutcome {
    RetriedInPlace,
    RestartRequired,
}

#[derive(Clone, Debug)]
pub struct EngineSnapshot {
    pub started: bool,
    pub stop_requested: bool,
    pub proxy_port: Option<u16>,
    pub route_stage: u8,
    pub route_hops: usize,
    pub bytes_up: u64,
    pub bytes_down: u64,
    pub last_exit_code: Option<i32>,
    pub running_for: Duration,
    pub last_connection_error: Option<String>,
    pub available_exit_countries: Vec<String>,
}

impl EngineHandle {
    pub fn start(config: &MobileConfig, wallet: &WalletMaterial) -> Result<Self, String> {
        let data_directory = config.data_directory.as_deref().ok_or_else(|| {
            "The native app did not provide a protected data directory.".to_owned()
        })?;
        fs::create_dir_all(data_directory)
            .map_err(|_| "The MASQ data directory could not be created.".to_owned())?;

        let (proxy_port, ui_port) = reserve_loopback_ports()?;
        let private_key = wallet.private_key_hex();
        let mut args = vec![
            "MASQNode".to_owned(),
            "--mobile-proxy-port".to_owned(),
            proxy_port.to_string(),
            "--ui-port".to_owned(),
            ui_port.to_string(),
            "--neighborhood-mode".to_owned(),
            "consume-only".to_owned(),
            "--chain".to_owned(),
            config.chain.identifier().to_owned(),
            "--blockchain-service-url".to_owned(),
            config.rpc_url.clone(),
            "--neighbors".to_owned(),
            config.neighbors.join(","),
            "--consuming-private-key".to_owned(),
            private_key.to_string(),
            "--data-directory".to_owned(),
            data_directory.to_owned(),
            "--log-level".to_owned(),
            "warn".to_owned(),
            "--min-hops".to_owned(),
            config.min_hops.to_string(),
        ];
        let secret_argument_index = 14;

        // Reset observable state before the thread is scheduled so a fast status poll can never
        // mistake the previous run's clean exit for a failure of this new run.
        node_lib::mobile_runtime::prepare(proxy_port);
        node_lib::mobile_runtime::set_exit_preference(
            config.exit_country.as_deref(),
            config.exit_country_fallback,
        );
        let thread = thread::Builder::new()
            .name("masq-node-consumer".to_owned())
            .spawn(move || {
                let exit_code = node_lib::mobile_runtime::run_embedded(&args);
                args[secret_argument_index].zeroize();
                exit_code
            })
            .map_err(|_| "The MASQ Node thread could not be started.".to_owned())?;

        Ok(Self {
            thread: Some(thread),
            proxy_port,
            min_hops: config.min_hops,
            started_at: Instant::now(),
        })
    }

    pub fn snapshot(&self) -> EngineSnapshot {
        let snapshot = node_lib::mobile_runtime::snapshot();
        EngineSnapshot {
            started: snapshot.started,
            stop_requested: snapshot.stop_requested,
            proxy_port: snapshot.proxy_port.or(Some(self.proxy_port)),
            route_stage: snapshot.route_stage,
            route_hops: usize::from(snapshot.route_hops),
            bytes_up: snapshot.bytes_up,
            bytes_down: snapshot.bytes_down,
            last_exit_code: snapshot.last_exit_code,
            running_for: self.started_at.elapsed(),
            last_connection_error: snapshot.last_connection_error,
            available_exit_countries: snapshot.available_exit_countries,
        }
    }

    pub fn set_min_hops(&mut self, min_hops: u8) {
        self.min_hops = min_hops;
    }

    pub fn retry_connection(
        &mut self,
        entry_nodes: &[String],
    ) -> Result<RetryConnectionOutcome, String> {
        if self.runtime_thread_has_ended() {
            self.reap_if_finished();
            return Ok(RetryConnectionOutcome::RestartRequired);
        }

        match node_lib::mobile_runtime::retry_connection(entry_nodes) {
            Ok(()) => {
                self.started_at = Instant::now();
                Ok(RetryConnectionOutcome::RetriedInPlace)
            }
            Err(_error) if self.runtime_thread_has_ended() => {
                self.reap_if_finished();
                Ok(RetryConnectionOutcome::RestartRequired)
            }
            Err(error) => Err(error),
        }
    }

    pub fn stop(&mut self) {
        node_lib::mobile_runtime::stop();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = node_lib::mobile_runtime::wait_for_actor_arbiters(Duration::from_secs(10));
        let _ = node_lib::mobile_runtime::wait_for_stream_connect_jobs(Duration::from_secs(10));
    }

    pub fn stop_with_timeout(&mut self, timeout: Duration) -> Result<(), String> {
        node_lib::mobile_runtime::stop();
        let deadline = Instant::now() + timeout;
        while self
            .thread
            .as_ref()
            .map(|thread| !thread.is_finished())
            .unwrap_or(false)
        {
            if Instant::now() >= deadline {
                return Err(
                    "The embedded MASQ Node did not stop in time. Direct browsing remains blocked."
                        .to_owned(),
                );
            }
            thread::sleep(Duration::from_millis(25));
        }
        let main_thread_error = if let Some(thread) = self.thread.take() {
            match thread.join() {
                Ok(0) => None,
                Ok(code) => Some(format!(
                        "The embedded MASQ Node stopped with code {code}. Direct browsing remains blocked."
                    )),
                Err(_) => Some(
                    "The embedded MASQ Node panicked while stopping. Direct browsing remains blocked."
                        .to_owned(),
                ),
            }
        } else {
            None
        };
        let remaining = deadline.saturating_duration_since(Instant::now());
        if !node_lib::mobile_runtime::wait_for_actor_arbiters(remaining) {
            return Err(
                "The embedded MASQ actor workers did not stop in time. Direct browsing remains blocked."
                    .to_owned(),
            );
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if !node_lib::mobile_runtime::wait_for_stream_connect_jobs(remaining) {
            return Err(
                "The embedded MASQ transport workers did not stop in time. Direct browsing remains blocked."
                    .to_owned(),
            );
        }
        main_thread_error.map_or(Ok(()), Err)
    }

    fn runtime_thread_has_ended(&self) -> bool {
        self.thread
            .as_ref()
            .map(JoinHandle::is_finished)
            .unwrap_or(true)
    }

    pub fn reap_if_finished(&mut self) {
        let finished = self
            .thread
            .as_ref()
            .map(|thread| thread.is_finished())
            .unwrap_or(false);
        if finished {
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }
}

impl Drop for EngineHandle {
    fn drop(&mut self) {
        if self.thread.is_some() {
            self.stop();
        }
    }
}

fn reserve_loopback_ports() -> Result<(u16, u16), String> {
    let proxy = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .map_err(|_| "No free local proxy port is available.".to_owned())?;
    let ui = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .map_err(|_| "No free local status port is available.".to_owned())?;
    let proxy_port = proxy
        .local_addr()
        .map_err(|_| "The local proxy port could not be read.".to_owned())?
        .port();
    let ui_port = ui
        .local_addr()
        .map_err(|_| "The local status port could not be read.".to_owned())?
        .port();
    drop((proxy, ui));
    Ok((proxy_port, ui_port))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Chain;
    use std::time::{Instant, SystemTime, UNIX_EPOCH};

    #[test]
    #[ignore = "opens loopback listeners for the embedded MASQ actor system"]
    fn embedded_node_starts_consume_only_and_stops_without_terminating_the_app() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let data_directory =
            std::env::temp_dir().join(format!("masq-mobile-smoke-{}-{unique}", std::process::id()));
        let config = MobileConfig {
            chain: Chain::BaseSepolia,
            rpc_url: "https://sepolia.base.org".to_owned(),
            neighbors: vec![
                "masq://base-sepolia:OHsC2CAm4rmfCkaFfiynwxflUgVTJRb2oY5mWxNCQkY@203.0.113.1:6642"
                    .to_owned(),
            ],
            min_hops: 1,
            exit_country: None,
            exit_country_fallback: true,
            data_directory: Some(data_directory.to_string_lossy().into_owned()),
        };
        let wallet = WalletMaterial::import(
            "0000000000000000000000000000000000000000000000000000000000000001",
        )
        .unwrap();

        let await_started = |engine: &EngineHandle| {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                let snapshot = engine.snapshot();
                if snapshot.started {
                    assert!(snapshot.proxy_port.is_some());
                    break;
                }
                if let Some(code) = snapshot.last_exit_code {
                    panic!("embedded MASQ Node exited during startup with code {code}");
                }
                assert!(
                    Instant::now() < deadline,
                    "embedded MASQ Node did not start in time"
                );
                thread::sleep(Duration::from_millis(50));
            }
        };

        for cycle in 1..=3 {
            let mut engine = EngineHandle::start(&config, &wallet)
                .unwrap_or_else(|error| panic!("embedded Node cycle {cycle} failed: {error}"));
            await_started(&engine);
            assert_eq!(
                node_lib::mobile_runtime::expected_actor_arbiter_count(),
                9,
                "embedded Node cycle {cycle} did not expect every consume-only actor worker"
            );
            assert_eq!(
                node_lib::mobile_runtime::tracked_actor_arbiter_count(),
                9,
                "embedded Node cycle {cycle} did not register every consume-only actor worker"
            );
            engine
                .stop_with_timeout(Duration::from_secs(10))
                .unwrap_or_else(|error| {
                    panic!("embedded Node cycle {cycle} did not stop: {error}")
                });
            assert_eq!(
                node_lib::mobile_runtime::snapshot().last_exit_code,
                Some(0),
                "embedded Node cycle {cycle} did not stop cleanly"
            );
        }
    }

    #[test]
    fn bounded_stop_reports_a_thread_that_does_not_finish_in_time() {
        let mut engine = EngineHandle {
            thread: Some(thread::spawn(|| {
                thread::sleep(Duration::from_millis(100));
                0
            })),
            proxy_port: 0,
            min_hops: 1,
            started_at: Instant::now(),
        };

        let result = engine.stop_with_timeout(Duration::from_millis(1));

        assert_eq!(
            result,
            Err(
                "The embedded MASQ Node did not stop in time. Direct browsing remains blocked."
                    .to_owned()
            )
        );
        engine.stop();
    }

    #[test]
    fn retry_requests_a_restart_only_after_the_runtime_thread_has_finished() {
        let thread = thread::spawn(|| 0);
        while !thread.is_finished() {
            thread::yield_now();
        }
        let mut engine = EngineHandle {
            thread: Some(thread),
            proxy_port: 0,
            min_hops: 1,
            started_at: Instant::now(),
        };

        let outcome = engine
            .retry_connection(&[])
            .expect("a finished runtime should be safely reaped");

        assert_eq!(outcome, RetryConnectionOutcome::RestartRequired);
        assert!(engine.thread.is_none());
    }
}
