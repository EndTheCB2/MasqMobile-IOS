mod lifecycle;

use std::ffi::c_void;
use std::future::Future;
use std::task::Poll;
use std::time::Duration;

use jni::EnvUnowned;
use jni::errors::ThrowRuntimeExAndDefault;
use jni::objects::{JClass, JString};
use jni::sys::{jboolean, jint};
use once_cell::sync::Lazy;
use tun2proxy::{
    ArgDns, ArgProxy, Args, CancellationToken, SessionPolicy, TrafficStatus, reset_session_metrics,
    session_metrics_snapshot, tun2proxy_set_traffic_status_callback,
};

use lifecycle::{
    BeginError, CompletionOutcome, LifecycleController, NativeTunnelSnapshot, RunCompletion,
};

const START_STOPPED: i32 = 0;
const START_FAILED: i32 = -1;
const START_UNEXPECTED_CLEAN_RETURN: i32 = -2;
const START_BUSY: i32 = -3;
const START_STALE_COMPLETION: i32 = -4;
const SESSION_CAPACITY: usize = 256;

static TUNNEL_LIFECYCLE: Lazy<LifecycleController> = Lazy::new(LifecycleController::default);

struct TrafficObservationContext {
    generation: u64,
}

struct TrafficCallbackRegistration {
    context: *mut TrafficObservationContext,
}

impl TrafficCallbackRegistration {
    fn new(generation: u64) -> Self {
        let context = Box::into_raw(Box::new(TrafficObservationContext { generation }));
        unsafe {
            tun2proxy_set_traffic_status_callback(
                1,
                Some(record_traffic_observation),
                context.cast::<c_void>(),
            );
        }
        Self { context }
    }
}

impl Drop for TrafficCallbackRegistration {
    fn drop(&mut self) {
        unsafe {
            // tun2proxy serializes registration with callback dispatch. Once
            // this returns no callback can retain or reuse the context.
            tun2proxy_set_traffic_status_callback(1, None, std::ptr::null_mut());
            drop(Box::from_raw(self.context));
        }
    }
}

unsafe extern "C" fn record_traffic_observation(
    status: *const TrafficStatus,
    context: *mut c_void,
) {
    let Some(status) = (unsafe { status.as_ref() }) else {
        return;
    };
    let Some(context) = (unsafe { context.cast::<TrafficObservationContext>().as_ref() }) else {
        return;
    };
    apply_traffic_observation(&TUNNEL_LIFECYCLE, status, context);
}

fn apply_traffic_observation(
    lifecycle: &LifecycleController,
    status: &TrafficStatus,
    context: &TrafficObservationContext,
) {
    lifecycle.record_payload(context.generation, status.tx, status.rx);
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
    // STARTING hides metrics until mark_running. Clear the global tun2proxy
    // counters now, before RUNNING can be published; tun2proxy repeats this
    // defensively at worker entry before it accepts any session.
    reset_session_metrics(SESSION_CAPACITY);

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
    // ipstack still parses captured IPv6, while the explicit session policy
    // below closes it before proxy setup or capacity accounting.
    args.ipv6_enabled = false;
    // The local MASQ HTTP CONNECT listener currently supports TCP/443 only.
    // Virtual DNS must remain available; every other UDP flow (including QUIC)
    // is rejected before it can consume a session slot.
    args.session_policy = SessionPolicy {
        reject_ipv6: true,
        reject_udp_except_virtual_dns: true,
        allowed_tcp_ports: vec![443],
        proxy_handshake_timeout: Some(Duration::from_secs(15)),
        tcp_idle_timeout: Some(Duration::from_secs(120)),
    };
    args.mtu = mtu;
    args.max_sessions = SESSION_CAPACITY;
    args.exit_on_fatal_error = false;

    let Ok(runtime) = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    else {
        return completion_code(TUNNEL_LIFECYCLE.complete(generation, RunCompletion::Failed));
    };
    let traffic_callback = TrafficCallbackRegistration::new(generation);
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
    drop(traffic_callback);
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

fn tunnel_state_json(snapshot: NativeTunnelSnapshot) -> String {
    let lifecycle_json = snapshot.lifecycle.to_json();
    let mut result = lifecycle_json
        .strip_suffix('}')
        .unwrap_or(&lifecycle_json)
        .to_owned();
    result.push_str(&format!(
        ",\"trafficObserved\":{},\"sessionMetrics\":{{\"sessionCapacity\":{},\"activeSessions\":{},\"peakSessions\":{},\"rejectedCapacity\":{},\"rejectedUdp\":{},\"rejectedIpv6\":{},\"rejectedNon443Tcp\":{},\"payloadTxBytes\":{},\"payloadRxBytes\":{}}}",
        snapshot.traffic_observed,
        snapshot.session_metrics.session_capacity,
        snapshot.session_metrics.active_sessions,
        snapshot.session_metrics.peak_sessions,
        snapshot.session_metrics.rejected_capacity,
        snapshot.session_metrics.rejected_udp,
        snapshot.session_metrics.rejected_ipv6,
        snapshot.session_metrics.rejected_tcp_port,
        snapshot.payload_tx_bytes,
        snapshot.payload_rx_bytes,
    ));
    result.push('}');
    result
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
    let state_json = tunnel_state_json(TUNNEL_LIFECYCLE.native_snapshot(session_metrics_snapshot));
    env.with_env(|env| JString::from_str(env, state_json))
        .resolve::<ThrowRuntimeExAndDefault>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use lifecycle::{LastResult, LifecycleSnapshot, TunnelState};
    use tun2proxy::SessionMetrics;

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
    fn traffic_callback_keeps_generation_scoped_directional_payload_totals() {
        let controller = LifecycleController::default();
        let first_generation = controller.begin(CancellationToken::new()).unwrap();
        assert!(controller.mark_running(first_generation));
        let first_context = TrafficObservationContext {
            generation: first_generation,
        };

        apply_traffic_observation(
            &controller,
            &TrafficStatus { tx: 17, rx: 0 },
            &first_context,
        );
        let upload = controller.native_snapshot(SessionMetrics::default);
        assert!(upload.traffic_observed);
        assert_eq!(upload.payload_tx_bytes, 17);
        assert_eq!(upload.payload_rx_bytes, 0);

        apply_traffic_observation(
            &controller,
            &TrafficStatus { tx: 17, rx: 29 },
            &first_context,
        );
        let returned = controller.native_snapshot(SessionMetrics::default);
        assert_eq!(returned.payload_tx_bytes, 17);
        assert_eq!(returned.payload_rx_bytes, 29);

        assert_eq!(
            controller.complete(first_generation, RunCompletion::Failed),
            CompletionOutcome::Failed
        );
        let second_generation = controller.begin(CancellationToken::new()).unwrap();
        assert!(controller.mark_running(second_generation));
        let second_context = TrafficObservationContext {
            generation: second_generation,
        };

        // A callback already queued by the old tunnel cannot be relabeled as
        // traffic from the new RUNNING generation.
        apply_traffic_observation(
            &controller,
            &TrafficStatus { tx: 999, rx: 999 },
            &first_context,
        );
        apply_traffic_observation(
            &controller,
            &TrafficStatus { tx: 5, rx: 0 },
            &second_context,
        );
        let second = controller.native_snapshot(SessionMetrics::default);
        assert_eq!(second.lifecycle.generation, second_generation);
        assert_eq!(second.payload_tx_bytes, 5);
        assert_eq!(second.payload_rx_bytes, 0);
    }

    #[test]
    fn state_json_reports_traffic_only_for_the_current_generation() {
        let snapshot = LifecycleSnapshot {
            state: TunnelState::Running,
            generation: 7,
            last_result: None,
        };
        let metrics = SessionMetrics {
            session_capacity: 256,
            active_sessions: 2,
            peak_sessions: 4,
            rejected_capacity: 1,
            rejected_udp: 11,
            rejected_ipv6: 3,
            rejected_tcp_port: 5,
        };

        assert_eq!(
            tunnel_state_json(NativeTunnelSnapshot {
                lifecycle: snapshot,
                traffic_observed: true,
                session_metrics: metrics,
                payload_tx_bytes: 17,
                payload_rx_bytes: 29,
            }),
            r#"{"state":"running","generation":7,"lastResult":null,"trafficObserved":true,"sessionMetrics":{"sessionCapacity":256,"activeSessions":2,"peakSessions":4,"rejectedCapacity":1,"rejectedUdp":11,"rejectedIpv6":3,"rejectedNon443Tcp":5,"payloadTxBytes":17,"payloadRxBytes":29}}"#,
        );
        assert_eq!(
            tunnel_state_json(NativeTunnelSnapshot {
                lifecycle: snapshot,
                traffic_observed: false,
                session_metrics: metrics,
                payload_tx_bytes: 0,
                payload_rx_bytes: 0,
            }),
            r#"{"state":"running","generation":7,"lastResult":null,"trafficObserved":false,"sessionMetrics":{"sessionCapacity":256,"activeSessions":2,"peakSessions":4,"rejectedCapacity":1,"rejectedUdp":11,"rejectedIpv6":3,"rejectedNon443Tcp":5,"payloadTxBytes":0,"payloadRxBytes":0}}"#,
        );
        assert_eq!(
            tunnel_state_json(NativeTunnelSnapshot {
                lifecycle: LifecycleSnapshot {
                    state: TunnelState::Idle,
                    generation: 0,
                    last_result: Some(LastResult::Stopped),
                },
                ..NativeTunnelSnapshot::default()
            }),
            r#"{"state":"idle","generation":0,"lastResult":"stopped","trafficObserved":false,"sessionMetrics":{"sessionCapacity":0,"activeSessions":0,"peakSessions":0,"rejectedCapacity":0,"rejectedUdp":0,"rejectedIpv6":0,"rejectedNon443Tcp":0,"payloadTxBytes":0,"payloadRxBytes":0}}"#,
        );
    }
}
