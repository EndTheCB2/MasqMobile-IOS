use crate::error::{Error, Result};
use std::os::raw::c_void;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

/// # Safety
///
/// set traffic status callback.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tun2proxy_set_traffic_status_callback(
    send_interval_secs: u32,
    callback: Option<unsafe extern "C" fn(*const TrafficStatus, *mut c_void)>,
    ctx: *mut c_void,
) {
    if let Ok(mut reporter) = TRAFFIC_REPORTER.lock() {
        // A callback registration is the embedding boundary between native
        // tunnel generations. Reset the aggregate and cadence while the same
        // lock that serializes updates and callback dispatch is held. Once
        // this returns, no callback from the previous registration can still
        // be running or begin later.
        reporter.callback = None;
        reporter.status = TrafficStatus::default();
        reporter.last_report_at = Instant::now();
        reporter.first_tx_reported = false;
        reporter.first_rx_reported = false;
        if send_interval_secs > 0 {
            reporter.send_interval = Duration::from_secs(send_interval_secs as u64);
        }
        reporter.callback = callback.map(|callback| TrafficStatusCallback(callback, ctx));
    } else {
        log::error!("set traffic status callback failed");
    }
}

#[repr(C)]
#[derive(Debug, Default, Copy, Clone, Eq, PartialEq)]
pub struct TrafficStatus {
    pub tx: u64,
    pub rx: u64,
}

#[derive(Clone)]
struct TrafficStatusCallback(unsafe extern "C" fn(*const TrafficStatus, *mut c_void), *mut c_void);

impl TrafficStatusCallback {
    unsafe fn call(self, info: &TrafficStatus) {
        unsafe { self.0(info, self.1) };
    }
}

unsafe impl Send for TrafficStatusCallback {}
unsafe impl Sync for TrafficStatusCallback {}

struct TrafficReporter {
    callback: Option<TrafficStatusCallback>,
    send_interval: Duration,
    status: TrafficStatus,
    last_report_at: Instant,
    first_tx_reported: bool,
    first_rx_reported: bool,
}

impl Default for TrafficReporter {
    fn default() -> Self {
        Self {
            callback: None,
            send_interval: Duration::from_secs(1),
            status: TrafficStatus::default(),
            last_report_at: Instant::now(),
            first_tx_reported: false,
            first_rx_reported: false,
        }
    }
}

static TRAFFIC_REPORTER: LazyLock<Mutex<TrafficReporter>> = LazyLock::new(|| Mutex::new(TrafficReporter::default()));

#[cfg(test)]
pub(crate) static TRAFFIC_STATUS_TEST_LOCK: Mutex<()> = Mutex::new(());

pub(crate) fn traffic_status_update(delta_tx: usize, delta_rx: usize) -> Result<()> {
    if delta_tx == 0 && delta_rx == 0 {
        return Ok(());
    }
    let mut reporter = TRAFFIC_REPORTER.lock().map_err(|e| Error::from(e.to_string()))?;
    if reporter.callback.is_none() {
        return Ok(());
    }

    reporter.status.tx = reporter.status.tx.saturating_add(delta_tx as u64);
    reporter.status.rx = reporter.status.rx.saturating_add(delta_rx as u64);
    let first_tx = delta_tx > 0 && !reporter.first_tx_reported;
    let first_rx = delta_rx > 0 && !reporter.first_rx_reported;
    reporter.first_tx_reported |= delta_tx > 0;
    reporter.first_rx_reported |= delta_rx > 0;

    let now = Instant::now();
    if first_tx || first_rx || now.duration_since(reporter.last_report_at) >= reporter.send_interval {
        reporter.last_report_at = now;
        let status = reporter.status;
        if let Some(callback) = reporter.callback.clone() {
            // Dispatch while holding the reporter lock. Traffic relays can
            // update from many Tokio tasks at once; serializing aggregation
            // and dispatch prevents an older cumulative snapshot from being
            // delivered after a newer one.
            unsafe { callback.call(&status) };
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier, mpsc};
    use std::thread;

    static CALLBACK_COUNT: AtomicUsize = AtomicUsize::new(0);
    static LAST_TX: AtomicU64 = AtomicU64::new(0);
    static LAST_RX: AtomicU64 = AtomicU64::new(0);
    static CALLBACK_HISTORY: Mutex<Vec<TrafficStatus>> = Mutex::new(Vec::new());

    unsafe extern "C" fn capture_status(status: *const TrafficStatus, _ctx: *mut c_void) {
        let Some(status) = (unsafe { status.as_ref() }) else {
            return;
        };
        LAST_TX.store(status.tx, Ordering::Relaxed);
        LAST_RX.store(status.rx, Ordering::Relaxed);
        CALLBACK_COUNT.fetch_add(1, Ordering::Relaxed);
    }

    unsafe extern "C" fn capture_status_history(status: *const TrafficStatus, _ctx: *mut c_void) {
        let Some(status) = (unsafe { status.as_ref() }) else {
            return;
        };
        CALLBACK_HISTORY.lock().expect("callback history").push(*status);
    }

    struct BlockingCallbackContext {
        entered: Barrier,
        release: Barrier,
    }

    unsafe extern "C" fn block_callback(_status: *const TrafficStatus, ctx: *mut c_void) {
        let context = unsafe { &*ctx.cast::<BlockingCallbackContext>() };
        context.entered.wait();
        context.release.wait();
    }

    #[test]
    fn first_real_payload_is_reported_immediately_for_each_registration() {
        let _lock = TRAFFIC_STATUS_TEST_LOCK.lock().expect("traffic status test lock");
        CALLBACK_COUNT.store(0, Ordering::Relaxed);
        LAST_TX.store(0, Ordering::Relaxed);
        LAST_RX.store(0, Ordering::Relaxed);

        unsafe {
            tun2proxy_set_traffic_status_callback(60, Some(capture_status), std::ptr::null_mut());
        }
        traffic_status_update(0, 0).expect("zero update");
        assert_eq!(CALLBACK_COUNT.load(Ordering::Relaxed), 0);

        traffic_status_update(7, 0).expect("first payload");
        assert_eq!(CALLBACK_COUNT.load(Ordering::Relaxed), 1);
        assert_eq!(LAST_TX.load(Ordering::Relaxed), 7);
        assert_eq!(LAST_RX.load(Ordering::Relaxed), 0);

        traffic_status_update(3, 0).expect("throttled payload");
        assert_eq!(CALLBACK_COUNT.load(Ordering::Relaxed), 1);

        traffic_status_update(0, 5).expect("first returned payload");
        assert_eq!(CALLBACK_COUNT.load(Ordering::Relaxed), 2);
        assert_eq!(LAST_TX.load(Ordering::Relaxed), 10);
        assert_eq!(LAST_RX.load(Ordering::Relaxed), 5);

        unsafe {
            tun2proxy_set_traffic_status_callback(60, Some(capture_status), std::ptr::null_mut());
        }
        traffic_status_update(0, 6).expect("next generation first payload");
        assert_eq!(CALLBACK_COUNT.load(Ordering::Relaxed), 3);
        assert_eq!(LAST_TX.load(Ordering::Relaxed), 0);
        assert_eq!(LAST_RX.load(Ordering::Relaxed), 6);

        unsafe {
            tun2proxy_set_traffic_status_callback(60, None, std::ptr::null_mut());
        }
    }

    #[test]
    fn concurrent_callbacks_never_regress_cumulative_directional_totals() {
        let _lock = TRAFFIC_STATUS_TEST_LOCK.lock().expect("traffic status test lock");
        CALLBACK_HISTORY.lock().expect("callback history").clear();
        unsafe {
            tun2proxy_set_traffic_status_callback(60, Some(capture_status_history), std::ptr::null_mut());
        }
        // Test-only zero cadence makes every update observable, exercising
        // callback ordering rather than only the two first-direction events.
        TRAFFIC_REPORTER.lock().expect("traffic reporter").send_interval = Duration::ZERO;

        const THREADS: usize = 12;
        const UPDATES_PER_THREAD: usize = 250;
        let barrier = Arc::new(Barrier::new(THREADS));
        let workers = (0..THREADS)
            .map(|worker| {
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    for update in 0..UPDATES_PER_THREAD {
                        if (worker + update) % 2 == 0 {
                            traffic_status_update(1, 0).expect("tx update");
                        } else {
                            traffic_status_update(0, 1).expect("rx update");
                        }
                    }
                })
            })
            .collect::<Vec<_>>();
        for worker in workers {
            worker.join().expect("traffic worker");
        }

        let history = CALLBACK_HISTORY.lock().expect("callback history");
        assert_eq!(history.len(), THREADS * UPDATES_PER_THREAD);
        assert!(
            history.windows(2).all(|pair| {
                pair[0].tx <= pair[1].tx && pair[0].rx <= pair[1].rx && (pair[0].tx < pair[1].tx || pair[0].rx < pair[1].rx)
            })
        );
        let expected_per_direction = (THREADS * UPDATES_PER_THREAD / 2) as u64;
        assert_eq!(
            history.last(),
            Some(&TrafficStatus {
                tx: expected_per_direction,
                rx: expected_per_direction
            })
        );
        drop(history);

        unsafe {
            tun2proxy_set_traffic_status_callback(60, None, std::ptr::null_mut());
        }
    }

    #[test]
    fn callback_re_registration_waits_for_in_flight_dispatch_to_finish() {
        let _lock = TRAFFIC_STATUS_TEST_LOCK.lock().expect("traffic status test lock");
        let context = BlockingCallbackContext {
            entered: Barrier::new(2),
            release: Barrier::new(2),
        };
        unsafe {
            tun2proxy_set_traffic_status_callback(60, Some(block_callback), std::ptr::from_ref(&context).cast_mut().cast::<c_void>());
        }

        let update = thread::spawn(|| traffic_status_update(1, 0).expect("payload update"));
        context.entered.wait();

        let (unregistered_tx, unregistered_rx) = mpsc::channel();
        let unregister = thread::spawn(move || {
            unsafe {
                tun2proxy_set_traffic_status_callback(60, None, std::ptr::null_mut());
            }
            unregistered_tx.send(()).unwrap();
        });
        assert!(unregistered_rx.recv_timeout(Duration::from_millis(50)).is_err());

        context.release.wait();
        update.join().unwrap();
        unregistered_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("registration waits, then completes");
        unregister.join().unwrap();
    }
}
