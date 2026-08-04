use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SessionMetrics {
    pub session_capacity: u64,
    pub active_sessions: u64,
    pub peak_sessions: u64,
    pub rejected_capacity: u64,
    pub rejected_udp: u64,
    pub rejected_ipv6: u64,
    pub rejected_tcp_port: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SessionRejection {
    Udp,
    Ipv6,
    TcpPort,
}

static SESSION_CAPACITY: AtomicU64 = AtomicU64::new(0);
static ACTIVE_SESSIONS: AtomicU64 = AtomicU64::new(0);
static PEAK_SESSIONS: AtomicU64 = AtomicU64::new(0);
static REJECTED_CAPACITY: AtomicU64 = AtomicU64::new(0);
static REJECTED_UDP: AtomicU64 = AtomicU64::new(0);
static REJECTED_IPV6: AtomicU64 = AtomicU64::new(0);
static REJECTED_TCP_PORT: AtomicU64 = AtomicU64::new(0);

#[cfg(test)]
pub(crate) static SESSION_METRICS_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub fn session_metrics_snapshot() -> SessionMetrics {
    SessionMetrics {
        session_capacity: SESSION_CAPACITY.load(Ordering::Relaxed),
        active_sessions: ACTIVE_SESSIONS.load(Ordering::Relaxed),
        peak_sessions: PEAK_SESSIONS.load(Ordering::Relaxed),
        rejected_capacity: REJECTED_CAPACITY.load(Ordering::Relaxed),
        rejected_udp: REJECTED_UDP.load(Ordering::Relaxed),
        rejected_ipv6: REJECTED_IPV6.load(Ordering::Relaxed),
        rejected_tcp_port: REJECTED_TCP_PORT.load(Ordering::Relaxed),
    }
}

/// Clears the process-wide session counters before a new, non-overlapping
/// tunnel generation is published to an embedding application's observers.
pub fn reset_session_metrics(capacity: usize) {
    SESSION_CAPACITY.store(u64::try_from(capacity).unwrap_or(u64::MAX), Ordering::Relaxed);
    ACTIVE_SESSIONS.store(0, Ordering::Relaxed);
    PEAK_SESSIONS.store(0, Ordering::Relaxed);
    REJECTED_CAPACITY.store(0, Ordering::Relaxed);
    REJECTED_UDP.store(0, Ordering::Relaxed);
    REJECTED_IPV6.store(0, Ordering::Relaxed);
    REJECTED_TCP_PORT.store(0, Ordering::Relaxed);
}

pub(crate) fn record_session_rejection(rejection: SessionRejection) {
    let counter = match rejection {
        SessionRejection::Udp => &REJECTED_UDP,
        SessionRejection::Ipv6 => &REJECTED_IPV6,
        SessionRejection::TcpPort => &REJECTED_TCP_PORT,
    };
    counter.fetch_add(1, Ordering::Relaxed);
}

/// RAII capacity permit. In particular, a proxy-handler construction error
/// drops this value and cannot leak one unit of session capacity.
pub(crate) struct SessionPermit;

impl SessionPermit {
    pub(crate) fn try_acquire(capacity: usize) -> Option<Self> {
        let capacity = u64::try_from(capacity).unwrap_or(u64::MAX);
        let acquired = ACTIVE_SESSIONS.fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| {
            (active < capacity).then_some(active + 1)
        });
        match acquired {
            Ok(previous) => {
                PEAK_SESSIONS.fetch_max(previous.saturating_add(1), Ordering::Relaxed);
                Some(Self)
            }
            Err(_) => {
                REJECTED_CAPACITY.fetch_add(1, Ordering::Relaxed);
                None
            }
        }
    }
}

impl Drop for SessionPermit {
    fn drop(&mut self) {
        let _ = ACTIVE_SESSIONS.fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| Some(active.saturating_sub(1)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permit_drop_after_setup_failure_restores_capacity() {
        let _lock = SESSION_METRICS_TEST_LOCK.lock().expect("test lock");
        reset_session_metrics(1);

        let failed_setup_permit = SessionPermit::try_acquire(1).expect("first permit");
        assert!(SessionPermit::try_acquire(1).is_none());
        drop(failed_setup_permit);
        let replacement = SessionPermit::try_acquire(1).expect("capacity must be restored");

        let during_replacement = session_metrics_snapshot();
        assert_eq!(during_replacement.active_sessions, 1);
        assert_eq!(during_replacement.peak_sessions, 1);
        assert_eq!(during_replacement.rejected_capacity, 1);
        drop(replacement);
        assert_eq!(session_metrics_snapshot().active_sessions, 0);
    }

    #[test]
    fn policy_rejections_do_not_consume_capacity() {
        let _lock = SESSION_METRICS_TEST_LOCK.lock().expect("test lock");
        reset_session_metrics(1);
        record_session_rejection(SessionRejection::Udp);
        record_session_rejection(SessionRejection::Ipv6);
        record_session_rejection(SessionRejection::TcpPort);

        let snapshot = session_metrics_snapshot();
        assert_eq!(snapshot.active_sessions, 0);
        assert_eq!(snapshot.rejected_udp, 1);
        assert_eq!(snapshot.rejected_ipv6, 1);
        assert_eq!(snapshot.rejected_tcp_port, 1);
        assert!(SessionPermit::try_acquire(1).is_some());
    }
}
