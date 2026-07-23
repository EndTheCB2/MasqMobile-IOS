use std::ffi::c_void;
use std::sync::Mutex;

use once_cell::sync::Lazy;
use tun2proxy::{ArgDns, ArgProxy, Args, CancellationToken};

static TUNNEL_CANCELLATION: Lazy<Mutex<Option<CancellationToken>>> =
    Lazy::new(|| Mutex::new(None));

fn run(tun_fd: i32, proxy_port: u16, mtu: u16) -> Result<(), String> {
    if tun_fd < 0 || proxy_port == 0 || !(1280..=9000).contains(&mtu) {
        return Err("Invalid packet-tunnel configuration.".to_owned());
    }

    let cancellation = CancellationToken::new();
    {
        let mut active = TUNNEL_CANCELLATION
            .lock()
            .map_err(|_| "The packet-tunnel state is unavailable.".to_owned())?;
        if active.is_some() {
            return Err("A MASQ packet tunnel is already active.".to_owned());
        }
        *active = Some(cancellation.clone());
    }

    let result = (|| {
        let proxy = format!("http://127.0.0.1:{proxy_port}");
        let mut args = Args::default();
        args.proxy = ArgProxy::try_from(proxy.as_str())
            .map_err(|_| "The local MASQ proxy address is invalid.".to_owned())?;
        args.tun_fd = Some(tun_fd);
        args.close_fd_on_drop = Some(false);
        args.setup = false;
        args.dns = ArgDns::Virtual;
        args.ipv6_enabled = true;
        args.mtu = mtu;
        args.max_sessions = 256;
        args.exit_on_fatal_error = false;

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|_| "The packet-tunnel runtime could not start.".to_owned())?;
        runtime
            .block_on(tun2proxy::general_run_async(
                args,
                mtu,
                false,
                cancellation,
            ))
            .map(|_| ())
            .map_err(|error| format!("The packet tunnel stopped: {error}"))
    })();

    if let Ok(mut active) = TUNNEL_CANCELLATION.lock() {
        active.take();
    }
    result
}

fn stop() -> bool {
    TUNNEL_CANCELLATION
        .lock()
        .ok()
        .and_then(|mut active| active.take())
        .map(|cancellation| {
            cancellation.cancel();
            true
        })
        .unwrap_or(false)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_masqmobile_MasqPacketTunnelJni_nativeStart(
    _env: *mut c_void,
    _class: *mut c_void,
    tun_fd: i32,
    proxy_port: i32,
    mtu: i32,
) -> i32 {
    let Ok(proxy_port) = u16::try_from(proxy_port) else {
        return -1;
    };
    let Ok(mtu) = u16::try_from(mtu) else {
        return -1;
    };
    match run(tun_fd, proxy_port, mtu) {
        Ok(()) => 0,
        Err(_) => -1,
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_masqmobile_MasqPacketTunnelJni_nativeStop(
    _env: *mut c_void,
    _class: *mut c_void,
) -> u8 {
    if stop() { 1 } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_configuration_without_starting_a_tunnel() {
        assert_eq!(
            run(-1, 0, 1000).unwrap_err(),
            "Invalid packet-tunnel configuration."
        );
        assert!(!stop());
    }
}
