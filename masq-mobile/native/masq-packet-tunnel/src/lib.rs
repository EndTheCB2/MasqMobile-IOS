mod lifecycle;

use std::future::Future;
use std::task::Poll;

use jni::EnvUnowned;
use jni::errors::ThrowRuntimeExAndDefault;
use jni::objects::{JClass, JString};
use jni::sys::{jboolean, jint};
use once_cell::sync::Lazy;
use tun2proxy::{ArgDns, ArgProxy, Args, CancellationToken};

use lifecycle::{BeginError, CompletionOutcome, LifecycleController, RunCompletion};

const START_STOPPED: i32 = 0;
const START_FAILED: i32 = -1;
const START_UNEXPECTED_CLEAN_RETURN: i32 = -2;
const START_BUSY: i32 = -3;
const START_STALE_COMPLETION: i32 = -4;

static TUNNEL_LIFECYCLE: Lazy<LifecycleController> = Lazy::new(LifecycleController::default);

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
    env.with_env(|env| JString::from_str(env, TUNNEL_LIFECYCLE.snapshot().to_json()))
        .resolve::<ThrowRuntimeExAndDefault>()
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
