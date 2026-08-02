mod lifecycle;

use std::ffi::c_void;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::Poll;

use jni::EnvUnowned;
use jni::errors::ThrowRuntimeExAndDefault;
use jni::objects::{JClass, JString};
use jni::sys::{jboolean, jint};
use once_cell::sync::Lazy;
use tun2proxy::{
    ArgDns, ArgProxy, Args, CancellationToken, TrafficStatus, tun2proxy_set_traffic_status_callback,
};

use lifecycle::{
    BeginError, CompletionOutcome, LifecycleController, LifecycleSnapshot, RunCompletion,
};

const START_STOPPED: i32 = 0;
const START_FAILED: i32 = -1;
const START_UNEXPECTED_CLEAN_RETURN: i32 = -2;
const START_BUSY: i32 = -3;
const START_STALE_COMPLETION: i32 = -4;

static TUNNEL_LIFECYCLE: Lazy<LifecycleController> = Lazy::new(LifecycleController::default);
static ACTIVE_TRAFFIC_GENERATION: AtomicU64 = AtomicU64::new(0);
static OBSERVED_TRAFFIC_GENERATION: AtomicU64 = AtomicU64::new(0);

unsafe extern "C" fn record_traffic_observation(
    status: *const TrafficStatus,
    _context: *mut c_void,
) {
    let Some(status) = (unsafe { status.as_ref() }) else {
        return;
    };
    if status.tx == 0 && status.rx == 0 {
        return;
    }
    let generation = ACTIVE_TRAFFIC_GENERATION.load(Ordering::Acquire);
    if generation > 0 {
        OBSERVED_TRAFFIC_GENERATION.store(generation, Ordering::Release);
    }
}

fn run(tun_fd: i32, proxy_port: u16, mtu: u16) -> i32 {
    if tun_fd < 0 || proxy_port == 0 || !(1280..=9000).contains(&mtu) {
        return START_FAILED;
    }

    let cancellation = CancellationToken::new();
    let generation = match TUNNEL_LIFECYCLE.begin(cancellation.clone()) {
        Ok(generation) => generation,
        Err(BeginError::Busy) => return START_BUSY,
        Err(BeginError::GenerationExhausted) => return START_FAILED,
    };

    let proxy = format!("http://127.0.0.1:{proxy_port}");
    let mut args = Args::default();
    let Ok(proxy) = ArgProxy::try_from(proxy.as_str()) else {
        return completion_code(TUNNEL_LIFECYCLE.complete(generation, RunCompletion::Failed));
    };
    args.proxy = proxy;
    args.tun_fd = Some(tun_fd);
    args.close_fd_on_drop = Some(false);
    args.setup = false;
    args.dns = ArgDns::Virtual;
    // Keep IPv6 captured by Android's blocking TUN, but do not translate it:
    // the current MASQ HTTP CONNECT adapter is intentionally limited to
    // IPv4 TCP/443 plus virtual DNS.
    args.ipv6_enabled = false;
    args.mtu = mtu;
    args.max_sessions = 256;
    args.exit_on_fatal_error = false;

    let Ok(runtime) = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    else {
        return completion_code(TUNNEL_LIFECYCLE.complete(generation, RunCompletion::Failed));
    };
    ACTIVE_TRAFFIC_GENERATION.store(generation, Ordering::Release);
    unsafe {
        tun2proxy_set_traffic_status_callback(
            1,
            Some(record_traffic_observation),
            std::ptr::null_mut(),
        );
    }
    let completion = runtime.block_on(async {
        let mut worker = Box::pin(tun2proxy::general_run_async(
            args,
            mtu,
            false,
            cancellation.clone(),
        ));
        // Poll the exact tun2proxy Future once under its runtime. tun2proxy
        // performs TUN creation and submits its run loop before its first
        // Pending. Only that deterministic Pending is our native readiness
        // handshake; an immediate Ready is initialization failure/exit.
        let immediate = std::future::poll_fn(|context| {
            Poll::Ready(match worker.as_mut().poll(context) {
                Poll::Ready(result) => Some(result),
                Poll::Pending => None,
            })
        })
        .await;
        if let Some(result) = immediate {
            return match result {
                Ok(_) => RunCompletion::Clean,
                Err(_) => RunCompletion::Failed,
            };
        }
        if !TUNNEL_LIFECYCLE.mark_running(generation) {
            cancellation.cancel();
        }
        match worker.await {
            Ok(_) => RunCompletion::Clean,
            Err(_) => RunCompletion::Failed,
        }
    });
    let _ = ACTIVE_TRAFFIC_GENERATION.compare_exchange(
        generation,
        0,
        Ordering::AcqRel,
        Ordering::Acquire,
    );
    completion_code(TUNNEL_LIFECYCLE.complete(generation, completion))
}

fn stop() -> bool {
    TUNNEL_LIFECYCLE.request_stop()
}

fn completion_code(outcome: CompletionOutcome) -> i32 {
    match outcome {
        CompletionOutcome::Stopped => START_STOPPED,
        CompletionOutcome::UnexpectedCleanReturn => START_UNEXPECTED_CLEAN_RETURN,
        CompletionOutcome::Failed => START_FAILED,
        CompletionOutcome::Stale => START_STALE_COMPLETION,
    }
}

fn tunnel_state_json(snapshot: LifecycleSnapshot, observed_generation: u64) -> String {
    let traffic_observed = snapshot.generation > 0 && observed_generation == snapshot.generation;
    let lifecycle_json = snapshot.to_json();
    format!(
        "{},\"trafficObserved\":{traffic_observed}}}",
        lifecycle_json.strip_suffix('}').unwrap_or(&lifecycle_json),
    )
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_masqmobile_MasqPacketTunnelJni_nativeStart<'local>(
    _env: EnvUnowned<'local>,
    _class: JClass<'local>,
    tun_fd: jint,
    proxy_port: jint,
    mtu: jint,
) -> jint {
    let Ok(proxy_port) = u16::try_from(proxy_port) else {
        return START_FAILED;
    };
    let Ok(mtu) = u16::try_from(mtu) else {
        return START_FAILED;
    };
    run(tun_fd, proxy_port, mtu)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_masqmobile_MasqPacketTunnelJni_nativeStop<'local>(
    _env: EnvUnowned<'local>,
    _class: JClass<'local>,
) -> jboolean {
    stop()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_masqmobile_MasqPacketTunnelJni_nativeStateJson<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
) -> JString<'local> {
    let snapshot = TUNNEL_LIFECYCLE.snapshot();
    let state_json = tunnel_state_json(
        snapshot,
        OBSERVED_TRAFFIC_GENERATION.load(Ordering::Acquire),
    );
    env.with_env(|env| JString::from_str(env, state_json))
        .resolve::<ThrowRuntimeExAndDefault>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use lifecycle::{LastResult, TunnelState};

    #[test]
    fn rejects_invalid_configuration_without_starting_a_tunnel() {
        assert_eq!(run(-1, 0, 1000), START_FAILED);
        assert!(!stop());
    }

    #[test]
    fn completion_outcomes_have_distinct_native_start_codes() {
        assert_eq!(completion_code(CompletionOutcome::Stopped), START_STOPPED);
        assert_eq!(
            completion_code(CompletionOutcome::UnexpectedCleanReturn),
            START_UNEXPECTED_CLEAN_RETURN
        );
        assert_eq!(completion_code(CompletionOutcome::Failed), START_FAILED);
        assert_eq!(
            completion_code(CompletionOutcome::Stale),
            START_STALE_COMPLETION
        );
    }

    #[test]
    fn state_json_reports_traffic_only_for_the_current_generation() {
        let snapshot = LifecycleSnapshot {
            state: TunnelState::Running,
            generation: 7,
            last_result: None,
        };

        assert_eq!(
            tunnel_state_json(snapshot, 7),
            r#"{"state":"running","generation":7,"lastResult":null,"trafficObserved":true}"#,
        );
        assert_eq!(
            tunnel_state_json(snapshot, 6),
            r#"{"state":"running","generation":7,"lastResult":null,"trafficObserved":false}"#,
        );
        assert_eq!(
            tunnel_state_json(
                LifecycleSnapshot {
                    state: TunnelState::Idle,
                    generation: 0,
                    last_result: Some(LastResult::Stopped),
                },
                0,
            ),
            r#"{"state":"idle","generation":0,"lastResult":"stopped","trafficObserved":false}"#,
        );
    }
}
