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
    with_core(MobileCore::preflight_proxy)
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

#[cfg(target_os = "android")]
mod android;
