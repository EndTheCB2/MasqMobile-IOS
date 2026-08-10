mod config;
mod core;
#[cfg(feature = "node-engine")]
mod engine;
pub mod proxy;
mod wallet;

use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::sync::Mutex;

use core::MobileCore;
use once_cell::sync::Lazy;
use zeroize::Zeroizing;

static CORE: Lazy<Mutex<MobileCore>> = Lazy::new(|| Mutex::new(MobileCore::default()));

/// Shared two-phase refresh boundary for the C ABI and Android JNI.
///
/// Only the tiny identity snapshots run under the mutex. The potentially
/// twelve-second socket/TLS probe runs after the first guard has been dropped,
/// allowing shutdown and recovery operations to acquire CORE immediately.
fn refresh_route_proof_status_with(
    core_mutex: &Mutex<MobileCore>,
    probe: impl FnOnce(u16) -> Result<(), String>,
) -> String {
    let ticket = {
        let mut core = core_mutex
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match core.begin_route_proof_refresh() {
            Some(ticket) => ticket,
            None => return core.route_proof_refresh_not_ready_json(),
        }
    };

    let probe_result = probe(ticket.proxy_port());
    core_mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .complete_route_proof_refresh(ticket, probe_result)
}

#[cfg(feature = "node-engine")]
pub(crate) fn refresh_route_proof_status() -> String {
    refresh_route_proof_status_with(&CORE, MobileCore::probe_private_route_for_refresh)
}

#[cfg(not(feature = "node-engine"))]
pub(crate) fn refresh_route_proof_status() -> String {
    CORE.lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .route_proof_refresh_unavailable_json()
}

fn with_core(operation: impl FnOnce(&mut MobileCore) -> Result<(), String>) -> *mut c_char {
    let mut core = CORE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Err(error) = operation(&mut core) {
        // Operations never include the secret input in their public error strings.
        core.record_error(error);
    }
    into_c_string(core.status_json())
}

fn with_core_value(
    operation: impl FnOnce(&mut MobileCore) -> Result<String, String>,
) -> *mut c_char {
    let mut core = CORE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let value = match operation(&mut core) {
        Ok(value) => value,
        Err(error) => serde_json::to_string(&serde_json::json!({ "error": error }))
            .expect("native error serialization is infallible"),
    };
    into_c_string(value)
}

fn read_argument(value: *const c_char) -> Result<String, String> {
    if value.is_null() {
        return Err("Missing argument.".to_owned());
    }
    unsafe { CStr::from_ptr(value) }
        .to_str()
        .map(str::to_owned)
        .map_err(|_| "Argument is not valid UTF-8.".to_owned())
}

fn into_c_string(value: String) -> *mut c_char {
    CString::new(value)
        .expect("serialized status contains no NUL")
        .into_raw()
}

#[no_mangle]
pub extern "C" fn masq_mobile_get_status() -> *mut c_char {
    with_core(|_| Ok(()))
}

#[no_mangle]
pub extern "C" fn masq_mobile_configure(config_json: *const c_char) -> *mut c_char {
    match read_argument(config_json) {
        Ok(config) => with_core(|core| core.configure(&config)),
        Err(_) => with_core(|_| Err("Invalid configuration.".to_owned())),
    }
}

#[no_mangle]
pub extern "C" fn masq_mobile_import_wallet(wallet_secret: *const c_char) -> *mut c_char {
    match read_argument(wallet_secret).map(Zeroizing::new) {
        Ok(wallet_secret) => with_core(|core| core.import_wallet(wallet_secret.as_str())),
        Err(_) => with_core(|_| Err("Invalid wallet secret.".to_owned())),
    }
}

#[no_mangle]
pub extern "C" fn masq_mobile_update_min_hops(min_hops: u8) -> *mut c_char {
    with_core(|core| core.update_min_hops(min_hops))
}

#[no_mangle]
pub extern "C" fn masq_mobile_start() -> *mut c_char {
    with_core(MobileCore::start)
}

#[no_mangle]
pub extern "C" fn masq_mobile_stop() -> *mut c_char {
    with_core(|core| {
        core.stop();
        Ok(())
    })
}

#[no_mangle]
pub extern "C" fn masq_mobile_shutdown() -> *mut c_char {
    with_core(|core| {
        core.shutdown();
        Ok(())
    })
}

#[no_mangle]
pub extern "C" fn masq_mobile_reset() -> *mut c_char {
    with_core(|core| {
        core.reset();
        Ok(())
    })
}

#[no_mangle]
pub extern "C" fn masq_mobile_reset_network_profile() -> *mut c_char {
    with_core(|core| {
        core.reset_network_profile();
        Ok(())
    })
}

#[no_mangle]
pub extern "C" fn masq_mobile_remove_wallet() -> *mut c_char {
    with_core(|core| {
        core.remove_wallet();
        Ok(())
    })
}

#[no_mangle]
pub extern "C" fn masq_mobile_preflight_proxy() -> *mut c_char {
    let status = CORE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .preflight_proxy_status_json();
    into_c_string(status)
}

#[no_mangle]
pub extern "C" fn masq_mobile_refresh_route_proof() -> *mut c_char {
    into_c_string(refresh_route_proof_status())
}

#[no_mangle]
pub extern "C" fn masq_mobile_set_proxy_enabled(enabled: bool) -> *mut c_char {
    with_core(|core| core.set_proxy_enabled(enabled))
}

#[no_mangle]
pub extern "C" fn masq_mobile_get_debt_summary() -> *mut c_char {
    with_core_value(|core| core.debt_summary_json())
}

#[no_mangle]
pub extern "C" fn masq_mobile_prepare_debt_settlement() -> *mut c_char {
    with_core_value(MobileCore::prepare_debt_settlement_json)
}

#[no_mangle]
pub extern "C" fn masq_mobile_confirm_debt_settlement(
    quote_id: *const c_char,
    maximum_masq_wei: *const c_char,
    maximum_estimated_l2_fee_wei: *const c_char,
) -> *mut c_char {
    let quote_id = read_argument(quote_id);
    let maximum_masq_wei = read_argument(maximum_masq_wei);
    let maximum_estimated_l2_fee_wei = read_argument(maximum_estimated_l2_fee_wei);
    with_core_value(|core| {
        core.confirm_debt_settlement_json(
            &quote_id?,
            &maximum_masq_wei?,
            &maximum_estimated_l2_fee_wei?,
        )
    })
}

#[no_mangle]
pub extern "C" fn masq_mobile_get_debt_settlement_status() -> *mut c_char {
    with_core_value(|core| core.debt_settlement_status_json())
}

#[no_mangle]
pub extern "C" fn masq_mobile_retry_debt_settlement() -> *mut c_char {
    with_core_value(MobileCore::retry_debt_settlement_json)
}

#[no_mangle]
pub unsafe extern "C" fn masq_mobile_string_free(value: *mut c_char) {
    if !value.is_null() {
        drop(CString::from_raw(value));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{mpsc, Arc};
    use std::thread;
    use std::time::Duration;

    use super::*;

    #[test]
    fn route_refresh_probe_releases_core_lock_and_stale_success_cannot_restore_shutdown_state() {
        let core_mutex = Arc::new(Mutex::new(MobileCore::healthy_for_route_refresh_test(
            7, 44_443,
        )));
        let refresh_core = Arc::clone(&core_mutex);
        let (probe_started_tx, probe_started_rx) = mpsc::channel();
        let (finish_probe_tx, finish_probe_rx) = mpsc::channel();

        let refresh = thread::spawn(move || {
            refresh_route_proof_status_with(&refresh_core, |port| {
                assert_eq!(port, 44_443);
                probe_started_tx.send(()).unwrap();
                finish_probe_rx
                    .recv_timeout(Duration::from_secs(1))
                    .expect("the test releases its bounded probe");
                Ok(())
            })
        });

        probe_started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("the refresh reached its unlocked probe phase");
        {
            let mut core = core_mutex
                .try_lock()
                .expect("CORE must not be locked by the route socket/TLS probe");
            core.shutdown();
        }
        finish_probe_tx.send(()).unwrap();

        let response: serde_json::Value = serde_json::from_str(&refresh.join().unwrap()).unwrap();
        assert_eq!(response["phase"], "unconfigured");
        assert_eq!(response["proxyEnabled"], false);
        assert_eq!(response["proxyPort"], serde_json::Value::Null);
        assert_eq!(response["routeStage"], 0);
        assert_eq!(response["routeProofRefresh"]["attempted"], false);
        assert_eq!(response["routeProofRefresh"]["succeeded"], false);
        assert_eq!(
            response["routeProofRefresh"]["errorCode"],
            "E_PRIVATE_ROUTE_REFRESH_NOT_READY"
        );
    }

    #[test]
    fn stale_probe_result_cannot_mutate_a_replacement_engine_with_the_same_proxy_port() {
        let core_mutex = Arc::new(Mutex::new(MobileCore::healthy_for_route_refresh_test(
            41, 44_443,
        )));
        let refresh_core = Arc::clone(&core_mutex);
        let (probe_started_tx, probe_started_rx) = mpsc::channel();
        let (finish_probe_tx, finish_probe_rx) = mpsc::channel();

        let refresh = thread::spawn(move || {
            refresh_route_proof_status_with(&refresh_core, |_| {
                probe_started_tx.send(()).unwrap();
                finish_probe_rx
                    .recv_timeout(Duration::from_secs(1))
                    .expect("the test releases its bounded probe");
                Ok(())
            })
        });

        probe_started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("the refresh reached its unlocked probe phase");
        *core_mutex.lock().unwrap() = MobileCore::healthy_for_route_refresh_test(42, 44_443);
        finish_probe_tx.send(()).unwrap();

        let response: serde_json::Value = serde_json::from_str(&refresh.join().unwrap()).unwrap();
        assert_eq!(response["engineGeneration"], 42);
        assert_eq!(response["proxyPort"], 44_443);
        assert_eq!(response["phase"], "connected");
        assert_eq!(response["routeProofRefresh"]["attempted"], false);
        assert_eq!(response["routeProofRefresh"]["succeeded"], false);
    }
}

#[cfg(target_os = "android")]
mod android;
