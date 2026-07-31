use std::sync::Mutex;

use tun2proxy::CancellationToken;

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

struct Lifecycle {
    generation: u64,
    state: TunnelState,
    active: Option<ActiveSession>,
    last_result: Option<LastResult>,
}

impl Default for Lifecycle {
    fn default() -> Self {
        Self {
            generation: 0,
            state: TunnelState::Idle,
            active: None,
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
        lifecycle.state = TunnelState::Running;
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
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier, mpsc::sync_channel};
    use std::thread;

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
}
