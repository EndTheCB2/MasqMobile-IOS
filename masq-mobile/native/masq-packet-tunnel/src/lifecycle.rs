use std::sync::Mutex;

use tun2proxy::{CancellationToken, SessionMetrics};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TunnelState {
    Idle,
    Starting,
    Running,
    Stopping,
    Failed,
}

impl TunnelState {
    fn wire_value(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Stopping => "stopping",
            Self::Failed => "failed",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LastResult {
    Stopped,
    UnexpectedCleanReturn,
    Failed,
}

impl LastResult {
    fn wire_value(self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::UnexpectedCleanReturn => "unexpectedCleanReturn",
            Self::Failed => "failed",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct LifecycleSnapshot {
    pub(crate) state: TunnelState,
    pub(crate) generation: u64,
    pub(crate) last_result: Option<LastResult>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct NativeTunnelSnapshot {
    pub(crate) lifecycle: LifecycleSnapshot,
    pub(crate) traffic_observed: bool,
    pub(crate) session_metrics: SessionMetrics,
    pub(crate) payload_tx_bytes: u64,
    pub(crate) payload_rx_bytes: u64,
}

impl Default for NativeTunnelSnapshot {
    fn default() -> Self {
        Self {
            lifecycle: LifecycleSnapshot {
                state: TunnelState::Idle,
                generation: 0,
                last_result: None,
            },
            traffic_observed: false,
            session_metrics: SessionMetrics::default(),
            payload_tx_bytes: 0,
            payload_rx_bytes: 0,
        }
    }
}

impl LifecycleSnapshot {
    pub(crate) fn to_json(self) -> String {
        let last_result = self
            .last_result
            .map(|result| format!("\"{}\"", result.wire_value()))
            .unwrap_or_else(|| "null".to_owned());
        format!(
            "{{\"state\":\"{}\",\"generation\":{},\"lastResult\":{last_result}}}",
            self.state.wire_value(),
            self.generation,
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BeginError {
    Busy,
    GenerationExhausted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RunCompletion {
    Clean,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CompletionOutcome {
    Stopped,
    UnexpectedCleanReturn,
    Failed,
    Stale,
}

struct ActiveSession {
    generation: u64,
    cancellation: CancellationToken,
}

struct GenerationObservability {
    generation: u64,
    metrics_ready: bool,
    payload_tx_bytes: u64,
    payload_rx_bytes: u64,
}

struct Lifecycle {
    generation: u64,
    state: TunnelState,
    active: Option<ActiveSession>,
    observability: Option<GenerationObservability>,
    last_result: Option<LastResult>,
}

impl Default for Lifecycle {
    fn default() -> Self {
        Self {
            generation: 0,
            state: TunnelState::Idle,
            active: None,
            observability: None,
            last_result: None,
        }
    }
}

#[derive(Default)]
pub(crate) struct LifecycleController {
    inner: Mutex<Lifecycle>,
}

impl LifecycleController {
    pub(crate) fn begin(&self, cancellation: CancellationToken) -> Result<u64, BeginError> {
        let mut lifecycle = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if lifecycle.active.is_some()
            || matches!(
                lifecycle.state,
                TunnelState::Starting | TunnelState::Running | TunnelState::Stopping
            )
        {
            return Err(BeginError::Busy);
        }

        let Some(generation) = lifecycle.generation.checked_add(1) else {
            lifecycle.state = TunnelState::Failed;
            lifecycle.last_result = Some(LastResult::Failed);
            return Err(BeginError::GenerationExhausted);
        };
        lifecycle.generation = generation;
        lifecycle.state = TunnelState::Starting;
        lifecycle.active = Some(ActiveSession {
            generation,
            cancellation,
        });
        // Reset the generation tag atomically with STARTING. tun2proxy resets
        // its own session counters during the first worker poll; metrics stay
        // hidden until mark_running confirms that initialization completed.
        lifecycle.observability = Some(GenerationObservability {
            generation,
            metrics_ready: false,
            payload_tx_bytes: 0,
            payload_rx_bytes: 0,
        });
        lifecycle.last_result = None;
        Ok(generation)
    }

    pub(crate) fn mark_running(&self, generation: u64) -> bool {
        let mut lifecycle = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let matches_generation = lifecycle
            .active
            .as_ref()
            .is_some_and(|active| active.generation == generation);
        if !matches_generation {
            return false;
        }
        if lifecycle.state != TunnelState::Starting {
            return false;
        }
        let Some(observability) = lifecycle
            .observability
            .as_mut()
            .filter(|observability| observability.generation == generation)
        else {
            return false;
        };
        // The caller marks the worker running only after tun2proxy's first
        // poll reset all generation-local session counters. Publishing this
        // tag and RUNNING under one lock prevents stale metrics from becoming
        // visible as counters for the new generation.
        observability.metrics_ready = true;
        lifecycle.state = TunnelState::Running;
        true
    }

    pub(crate) fn record_payload(&self, generation: u64, tx: u64, rx: u64) -> bool {
        if tx == 0 && rx == 0 {
            return false;
        }
        let mut lifecycle = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let matches_active_generation = lifecycle
            .active
            .as_ref()
            .is_some_and(|active| active.generation == generation);
        if !matches_active_generation {
            return false;
        }
        let Some(observability) = lifecycle
            .observability
            .as_mut()
            .filter(|observability| observability.generation == generation)
        else {
            return false;
        };
        // The callback supplies generation-local cumulative totals. max is a
        // defensive fence against any stale or duplicated callback delivery.
        observability.payload_tx_bytes = observability.payload_tx_bytes.max(tx);
        observability.payload_rx_bytes = observability.payload_rx_bytes.max(rx);
        true
    }

    pub(crate) fn request_stop(&self) -> bool {
        let cancellation = {
            let mut lifecycle = self
                .inner
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(cancellation) = lifecycle
                .active
                .as_ref()
                .map(|active| active.cancellation.clone())
            else {
                return false;
            };
            lifecycle.state = TunnelState::Stopping;
            cancellation
        };
        cancellation.cancel();
        true
    }

    pub(crate) fn complete(&self, generation: u64, completion: RunCompletion) -> CompletionOutcome {
        let mut lifecycle = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(active) = lifecycle
            .active
            .as_ref()
            .filter(|active| active.generation == generation)
        else {
            return CompletionOutcome::Stale;
        };
        let stop_requested =
            lifecycle.state == TunnelState::Stopping || active.cancellation.is_cancelled();

        lifecycle.active = None;
        lifecycle.observability = None;
        if stop_requested {
            lifecycle.state = TunnelState::Idle;
            lifecycle.last_result = Some(LastResult::Stopped);
            return CompletionOutcome::Stopped;
        }

        lifecycle.state = TunnelState::Failed;
        match completion {
            RunCompletion::Clean => {
                lifecycle.last_result = Some(LastResult::UnexpectedCleanReturn);
                CompletionOutcome::UnexpectedCleanReturn
            }
            RunCompletion::Failed => {
                lifecycle.last_result = Some(LastResult::Failed);
                CompletionOutcome::Failed
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn snapshot(&self) -> LifecycleSnapshot {
        let lifecycle = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        LifecycleSnapshot {
            state: lifecycle.state,
            generation: lifecycle.generation,
            last_result: lifecycle.last_result,
        }
    }

    pub(crate) fn native_snapshot<F>(&self, session_metrics_snapshot: F) -> NativeTunnelSnapshot
    where
        F: FnOnce() -> SessionMetrics,
    {
        let lifecycle = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let lifecycle_snapshot = LifecycleSnapshot {
            state: lifecycle.state,
            generation: lifecycle.generation,
            last_result: lifecycle.last_result,
        };
        let current_observability = lifecycle.observability.as_ref().filter(|observability| {
            observability.generation == lifecycle.generation
                && observability.metrics_ready
                && lifecycle
                    .active
                    .as_ref()
                    .is_some_and(|active| active.generation == observability.generation)
                && matches!(
                    lifecycle.state,
                    TunnelState::Running | TunnelState::Stopping
                )
        });
        let Some(observability) = current_observability else {
            return NativeTunnelSnapshot {
                lifecycle: lifecycle_snapshot,
                ..NativeTunnelSnapshot::default()
            };
        };

        // Evaluate the external atomic counters while the lifecycle lock is
        // held. A completion/new begin cannot cross this read, and a new
        // tun2proxy generation cannot reset its counters until begin returns.
        NativeTunnelSnapshot {
            lifecycle: lifecycle_snapshot,
            traffic_observed: observability.payload_tx_bytes > 0
                || observability.payload_rx_bytes > 0,
            session_metrics: session_metrics_snapshot(),
            payload_tx_bytes: observability.payload_tx_bytes,
            payload_rx_bytes: observability.payload_rx_bytes,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier, mpsc, mpsc::sync_channel};
    use std::thread;
    use std::time::Duration;

    use super::*;

    #[test]
    fn rapid_start_stop_start_keeps_the_first_session_owned_until_completion() {
        let controller = Arc::new(LifecycleController::default());
        let (running_sender, running_receiver) = sync_channel(0);
        let (finish_sender, finish_receiver) = sync_channel(0);
        let worker_controller = Arc::clone(&controller);
        let worker = thread::spawn(move || {
            let cancellation = CancellationToken::new();
            let generation = worker_controller.begin(cancellation.clone()).unwrap();
            assert!(worker_controller.mark_running(generation));
            running_sender.send((generation, cancellation)).unwrap();
            finish_receiver.recv().unwrap();
            worker_controller.complete(generation, RunCompletion::Clean)
        });

        let (first_generation, first_cancellation) = running_receiver.recv().unwrap();
        assert!(controller.request_stop());
        assert!(first_cancellation.is_cancelled());
        assert_eq!(
            controller.snapshot(),
            LifecycleSnapshot {
                state: TunnelState::Stopping,
                generation: first_generation,
                last_result: None,
            }
        );

        let replacement_cancellation = CancellationToken::new();
        assert_eq!(
            controller.begin(replacement_cancellation.clone()),
            Err(BeginError::Busy)
        );
        assert!(!replacement_cancellation.is_cancelled());
        assert_eq!(controller.snapshot().generation, first_generation);

        finish_sender.send(()).unwrap();
        assert_eq!(worker.join().unwrap(), CompletionOutcome::Stopped);
        assert_eq!(controller.snapshot().state, TunnelState::Idle);

        let second_generation = controller
            .begin(CancellationToken::new())
            .expect("a new generation starts only after the old run returned");
        assert_eq!(second_generation, first_generation + 1);
        assert!(controller.mark_running(second_generation));
        assert!(controller.request_stop());
        assert_eq!(
            controller.complete(second_generation, RunCompletion::Clean),
            CompletionOutcome::Stopped
        );
    }

    #[test]
    fn stale_completion_cannot_remove_or_cancel_the_new_generation() {
        let controller = Arc::new(LifecycleController::default());
        let first_generation = controller.begin(CancellationToken::new()).unwrap();
        assert!(controller.mark_running(first_generation));
        assert!(controller.request_stop());
        assert_eq!(
            controller.complete(first_generation, RunCompletion::Clean),
            CompletionOutcome::Stopped
        );

        let current_cancellation = CancellationToken::new();
        let current_generation = controller.begin(current_cancellation.clone()).unwrap();
        assert!(controller.mark_running(current_generation));

        let barrier = Arc::new(Barrier::new(2));
        let stale_controller = Arc::clone(&controller);
        let stale_barrier = Arc::clone(&barrier);
        let stale_worker = thread::spawn(move || {
            stale_barrier.wait();
            stale_controller.complete(first_generation, RunCompletion::Clean)
        });
        barrier.wait();
        assert_eq!(stale_worker.join().unwrap(), CompletionOutcome::Stale);

        assert!(!current_cancellation.is_cancelled());
        assert_eq!(
            controller.snapshot(),
            LifecycleSnapshot {
                state: TunnelState::Running,
                generation: current_generation,
                last_result: None,
            }
        );
        assert!(controller.request_stop());
        assert_eq!(
            controller.complete(current_generation, RunCompletion::Clean),
            CompletionOutcome::Stopped
        );
    }

    #[test]
    fn unexpected_clean_return_is_distinct_from_failure_and_expected_stop() {
        let controller = LifecycleController::default();
        let clean_generation = controller.begin(CancellationToken::new()).unwrap();
        assert!(controller.mark_running(clean_generation));
        assert_eq!(
            controller.complete(clean_generation, RunCompletion::Clean),
            CompletionOutcome::UnexpectedCleanReturn
        );
        assert_eq!(
            controller.snapshot(),
            LifecycleSnapshot {
                state: TunnelState::Failed,
                generation: clean_generation,
                last_result: Some(LastResult::UnexpectedCleanReturn),
            }
        );

        let failed_generation = controller.begin(CancellationToken::new()).unwrap();
        assert!(controller.mark_running(failed_generation));
        assert_eq!(
            controller.complete(failed_generation, RunCompletion::Failed),
            CompletionOutcome::Failed
        );
        assert_eq!(
            controller.snapshot(),
            LifecycleSnapshot {
                state: TunnelState::Failed,
                generation: failed_generation,
                last_result: Some(LastResult::Failed),
            }
        );
    }

    #[test]
    fn a_stop_requested_during_initialization_cannot_be_marked_ready() {
        let controller = LifecycleController::default();
        let generation = controller.begin(CancellationToken::new()).unwrap();

        assert!(controller.request_stop());
        assert!(!controller.mark_running(generation));
        assert_eq!(controller.snapshot().state, TunnelState::Stopping);
        assert_eq!(
            controller.complete(generation, RunCompletion::Clean),
            CompletionOutcome::Stopped
        );
    }

    #[test]
    fn lifecycle_snapshot_has_a_stable_json_shape() {
        let controller = LifecycleController::default();
        assert_eq!(
            controller.snapshot().to_json(),
            r#"{"state":"idle","generation":0,"lastResult":null}"#
        );

        let generation = controller.begin(CancellationToken::new()).unwrap();
        assert_eq!(
            controller.snapshot().to_json(),
            format!(r#"{{"state":"starting","generation":{generation},"lastResult":null}}"#)
        );
    }

    #[test]
    fn metrics_and_payload_are_exposed_only_for_the_tagged_running_generation() {
        let controller = LifecycleController::default();
        let first_generation = controller.begin(CancellationToken::new()).unwrap();
        assert!(controller.record_payload(first_generation, 17, 0));

        let starting =
            controller.native_snapshot(|| panic!("STARTING must not read old session metrics"));
        assert_eq!(starting.lifecycle.state, TunnelState::Starting);
        assert_eq!(starting.lifecycle.generation, first_generation);
        assert_eq!(starting.session_metrics, SessionMetrics::default());
        assert!(!starting.traffic_observed);

        assert!(controller.mark_running(first_generation));
        assert!(controller.record_payload(first_generation, 17, 29));
        let first = controller.native_snapshot(|| SessionMetrics {
            session_capacity: 256,
            active_sessions: 2,
            ..SessionMetrics::default()
        });
        assert_eq!(first.lifecycle.generation, first_generation);
        assert_eq!(first.session_metrics.active_sessions, 2);
        assert_eq!(first.payload_tx_bytes, 17);
        assert_eq!(first.payload_rx_bytes, 29);
        assert!(first.traffic_observed);

        assert_eq!(
            controller.complete(first_generation, RunCompletion::Failed),
            CompletionOutcome::Failed
        );
        let second_generation = controller.begin(CancellationToken::new()).unwrap();
        assert_eq!(second_generation, first_generation + 1);
        assert!(!controller.record_payload(first_generation, 999, 999));
        assert!(controller.mark_running(second_generation));

        let second = controller.native_snapshot(|| SessionMetrics {
            session_capacity: 256,
            active_sessions: 1,
            ..SessionMetrics::default()
        });
        assert_eq!(second.lifecycle.generation, second_generation);
        assert_eq!(second.session_metrics.active_sessions, 1);
        assert!(!second.traffic_observed);
        assert_eq!(second.payload_tx_bytes, 0);
        assert_eq!(second.payload_rx_bytes, 0);
    }

    #[test]
    fn lifecycle_transition_cannot_cross_a_generation_tagged_metrics_snapshot() {
        let controller = Arc::new(LifecycleController::default());
        let first_generation = controller.begin(CancellationToken::new()).unwrap();
        assert!(controller.mark_running(first_generation));
        assert!(controller.record_payload(first_generation, 10, 20));

        let (snapshot_entered_tx, snapshot_entered_rx) = sync_channel(0);
        let (release_snapshot_tx, release_snapshot_rx) = sync_channel(0);
        let reader_controller = Arc::clone(&controller);
        let reader = thread::spawn(move || {
            reader_controller.native_snapshot(|| {
                snapshot_entered_tx.send(()).unwrap();
                release_snapshot_rx.recv().unwrap();
                SessionMetrics {
                    session_capacity: 256,
                    active_sessions: 3,
                    ..SessionMetrics::default()
                }
            })
        });
        snapshot_entered_rx.recv().unwrap();

        let (transition_attempted_tx, transition_attempted_rx) = sync_channel(0);
        let (transition_done_tx, transition_done_rx) = mpsc::channel();
        let writer_controller = Arc::clone(&controller);
        let writer = thread::spawn(move || {
            transition_attempted_tx.send(()).unwrap();
            assert_eq!(
                writer_controller.complete(first_generation, RunCompletion::Failed),
                CompletionOutcome::Failed
            );
            let next_generation = writer_controller.begin(CancellationToken::new()).unwrap();
            assert!(writer_controller.mark_running(next_generation));
            transition_done_tx.send(next_generation).unwrap();
        });
        transition_attempted_rx.recv().unwrap();
        assert!(
            transition_done_rx
                .recv_timeout(Duration::from_millis(50))
                .is_err()
        );

        release_snapshot_tx.send(()).unwrap();
        let first_snapshot = reader.join().unwrap();
        assert_eq!(first_snapshot.lifecycle.generation, first_generation);
        assert_eq!(first_snapshot.session_metrics.active_sessions, 3);
        assert_eq!(first_snapshot.payload_tx_bytes, 10);
        assert_eq!(first_snapshot.payload_rx_bytes, 20);

        let second_generation = transition_done_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        writer.join().unwrap();
        let second_snapshot = controller.native_snapshot(|| SessionMetrics {
            session_capacity: 256,
            active_sessions: 1,
            ..SessionMetrics::default()
        });
        assert_eq!(second_snapshot.lifecycle.generation, second_generation);
        assert_eq!(second_snapshot.session_metrics.active_sessions, 1);
        assert_eq!(second_snapshot.payload_tx_bytes, 0);
        assert_eq!(second_snapshot.payload_rx_bytes, 0);
    }
}
