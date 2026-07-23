use jni::objects::{JClass, JString};
use jni::sys::{jboolean, jint, jstring};
use jni::JNIEnv;
use zeroize::Zeroizing;

use crate::core::MobileCore;
use crate::CORE;

fn status_after(operation: impl FnOnce(&mut MobileCore) -> Result<(), String>) -> String {
    let mut core = CORE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Err(error) = operation(&mut core) {
        core.record_error(error);
    }
    core.status_json()
}

fn java_string(env: &mut JNIEnv<'_>, value: String) -> jstring {
    env.new_string(value)
        .expect("Android VM must allocate MASQ status string")
        .into_raw()
}

fn rust_string(env: &mut JNIEnv<'_>, value: JString<'_>) -> Result<String, String> {
    env.get_string(&value)
        .map(|value| value.into())
        .map_err(|_| "Invalid Java string.".to_owned())
}

#[no_mangle]
pub extern "system" fn Java_com_masqmobile_MasqCoreJni_nativeGetStatus(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jstring {
    let status = status_after(|_| Ok(()));
    java_string(&mut env, status)
}

#[no_mangle]
pub extern "system" fn Java_com_masqmobile_MasqCoreJni_nativeConfigure(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    value: JString<'_>,
) -> jstring {
    let result = rust_string(&mut env, value);
    let status = status_after(|core| result.and_then(|value| core.configure(&value)));
    java_string(&mut env, status)
}

#[no_mangle]
pub extern "system" fn Java_com_masqmobile_MasqCoreJni_nativeImportWallet(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    value: JString<'_>,
) -> jstring {
    let result = rust_string(&mut env, value).map(Zeroizing::new);
    let status = status_after(|core| result.and_then(|value| core.import_wallet(value.as_str())));
    java_string(&mut env, status)
}

#[no_mangle]
pub extern "system" fn Java_com_masqmobile_MasqCoreJni_nativeUpdateMinHops(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    min_hops: jint,
) -> jstring {
    let status = status_after(|core| {
        u8::try_from(min_hops)
            .map_err(|_| "Choose between one and six MASQ hops.".to_owned())
            .and_then(|min_hops| core.update_min_hops(min_hops))
    });
    java_string(&mut env, status)
}

#[no_mangle]
pub extern "system" fn Java_com_masqmobile_MasqCoreJni_nativeStart(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jstring {
    let status = status_after(MobileCore::start);
    java_string(&mut env, status)
}

#[no_mangle]
pub extern "system" fn Java_com_masqmobile_MasqCoreJni_nativeStop(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jstring {
    let status = status_after(|core| {
        core.stop();
        Ok(())
    });
    java_string(&mut env, status)
}

#[no_mangle]
pub extern "system" fn Java_com_masqmobile_MasqCoreJni_nativeShutdown(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jstring {
    let status = status_after(|core| {
        core.shutdown();
        Ok(())
    });
    java_string(&mut env, status)
}

#[no_mangle]
pub extern "system" fn Java_com_masqmobile_MasqCoreJni_nativeReset(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jstring {
    let status = status_after(|core| {
        core.reset();
        Ok(())
    });
    java_string(&mut env, status)
}

#[no_mangle]
pub extern "system" fn Java_com_masqmobile_MasqCoreJni_nativeResetNetworkProfile(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jstring {
    let status = status_after(|core| {
        core.reset_network_profile();
        Ok(())
    });
    java_string(&mut env, status)
}

#[no_mangle]
pub extern "system" fn Java_com_masqmobile_MasqCoreJni_nativeRemoveWallet(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jstring {
    let status = status_after(|core| {
        core.remove_wallet();
        Ok(())
    });
    java_string(&mut env, status)
}

#[no_mangle]
pub extern "system" fn Java_com_masqmobile_MasqCoreJni_nativePreflightProxy(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jstring {
    let status = status_after(MobileCore::preflight_proxy);
    java_string(&mut env, status)
}

#[no_mangle]
pub extern "system" fn Java_com_masqmobile_MasqCoreJni_nativeSetProxyEnabled(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    enabled: jboolean,
) -> jstring {
    let status = status_after(|core| core.set_proxy_enabled(enabled != 0));
    java_string(&mut env, status)
}
