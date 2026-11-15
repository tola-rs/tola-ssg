//! Shared hook permits for one site session.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::time::Duration;

use crate::cancellation::{BuildCancellation, BuildCancelled, BuildCanceller};
use crate::config::section::build::HookJobs;

#[derive(Debug, Default)]
struct State {
    epoch: u64,
    limit: usize,
    active: usize,
    draining: bool,
    next_batch: u64,
    // One queue entry per batch with work left, rather than one per hook. Rotating after each
    // grant prevents a large after-publish batch from filling the queue ahead of a new build.
    waiting: VecDeque<u64>,
}

#[derive(Debug)]
struct Pool {
    state: Mutex<State>,
    changed: Condvar,
    auto_limit: usize,
}

/// Permit pool shared by the site's modules, build attempts, and after-publish consumers.
/// Clones share the current limit; independent site sessions do not. Updating the limit can
/// change overlap in an older consumer, but never its owner boundaries or order batches.
#[derive(Clone, Debug)]
pub struct HookExecutor {
    pool: Arc<Pool>,
}

impl Default for HookExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl HookExecutor {
    /// Start a site session, sampling the automatic concurrency limit once.
    pub fn new() -> Self {
        let auto_limit = std::thread::available_parallelism().map_or(1, |n| n.get());
        Self {
            pool: Arc::new(Pool {
                state: Mutex::new(State {
                    limit: auto_limit,
                    ..State::default()
                }),
                changed: Condvar::new(),
                auto_limit,
            }),
        }
    }

    /// Advance the generation allowed to install settings and return it to the new attempt.
    /// This does not cancel already running work or alter the current limit.
    pub fn begin_attempt(&self) -> u64 {
        let mut state = self
            .pool
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.epoch = state
            .epoch
            .checked_add(1)
            .expect("hook attempt generation overflow");
        state.epoch
    }

    /// Revoke an attempt's right to install settings before the host requests its cancellation.
    pub fn invalidate(&self, epoch: u64) {
        let mut state = self
            .pool
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if state.epoch == epoch {
            state.epoch = state
                .epoch
                .checked_add(1)
                .expect("hook attempt generation overflow");
        }
    }

    /// Adopt settings after preflight, only while the attempt is still current and active.
    /// Lowering the limit never revokes granted work. The new limit also governs older
    /// after-publish consumers and remains adopted if this attempt later fails.
    pub fn configure(
        &self,
        epoch: u64,
        par_hooks: bool,
        jobs: HookJobs,
        cancellation: &BuildCancellation,
    ) -> Result<(), BuildCancelled> {
        let mut state = self
            .pool
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if epoch != state.epoch || cancellation.is_cancelled() {
            return Err(BuildCancelled);
        }
        let limit = super::hook_limit(par_hooks, jobs, self.pool.auto_limit);
        if limit == 1 && state.limit != 1 && state.active > 1 {
            state.draining = true;
        } else if limit > 1 {
            state.draining = false;
        }
        state.limit = limit;
        self.pool.changed.notify_all();
        Ok(())
    }

    /// Run one eligible order batch; the caller waits here before advancing to the next batch.
    /// Results retain declaration order, `None` means no startup grant, and an I/O error means
    /// the granted worker failed. The callback closes this batch immediately on observed
    /// failure, independently of the attempt's cancellation and other batches in this pool.
    pub(crate) fn run_batch<R: Send>(
        &self,
        count: usize,
        cancellation: Option<&BuildCancellation>,
        run: impl Fn(usize, &BuildCancellation, &Arc<dyn Fn() + Send + Sync>) -> R + Sync,
        failed: impl Fn(&R) -> bool + Sync,
    ) -> Vec<Option<std::io::Result<R>>> {
        let mut results = (0..count).map(|_| None).collect::<Vec<_>>();
        if count == 0 {
            return results;
        }
        let canceller = cancellation.map_or_else(BuildCanceller::new, BuildCanceller::child);
        let signal = canceller.token();
        let id = {
            let mut state = self
                .pool
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let id = state.next_batch;
            state.next_batch = state
                .next_batch
                .checked_add(1)
                .expect("hook batch generation overflow");
            state.waiting.push_back(id);
            self.pool.changed.notify_all();
            id
        };
        let dispatch = tracing::dispatcher::get_default(Clone::clone);
        let span = tracing::Span::current();
        std::thread::scope(|scope| {
            let (send, receive) = mpsc::channel();
            let mut next = 0;
            let mut running = 0;
            loop {
                while let Ok((index, result)) = receive.try_recv() {
                    match result {
                        Ok(result) => results[index] = Some(Ok(result)),
                        Err(_) => {
                            results[index] = Some(Err(std::io::Error::other(
                                "the hook worker stopped unexpectedly",
                            )));
                        }
                    }
                    running -= 1;
                }
                let mut state = self
                    .pool
                    .state
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if signal.is_cancelled() {
                    state.waiting.retain(|batch| *batch != id);
                    self.pool.changed.notify_all();
                    if running == 0 {
                        break;
                    }
                } else if next == count {
                    state.waiting.retain(|batch| *batch != id);
                    if running == 0 {
                        break;
                    }
                } else if state.waiting.front() == Some(&id)
                    && !state.draining
                    && state.active < state.limit
                {
                    state.waiting.pop_front();
                    next += 1;
                    let index = next - 1;
                    if next < count {
                        state.waiting.push_back(id);
                    }
                    // A grant counts before thread creation. Hot shrinking cannot revoke it;
                    // the permit remains held through command, report, and output cleanup.
                    state.active += 1;
                    self.pool.changed.notify_all();
                    drop(state);
                    let permit = Permit { pool: &self.pool };
                    let sender = send.clone();
                    let signal = &signal;
                    let run = &run;
                    let failed = &failed;
                    let pool = Arc::clone(&self.pool);
                    let stop_batch = canceller.clone();
                    let worker_dispatch = dispatch.clone();
                    let worker_span = span.clone();
                    let stop: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
                        let _state = pool.state.lock().unwrap_or_else(|error| error.into_inner());
                        stop_batch.cancel();
                        pool.changed.notify_all();
                    });
                    let spawned = std::thread::Builder::new().spawn_scoped(scope, move || {
                        let _permit = permit;
                        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            tracing::dispatcher::with_default(&worker_dispatch, || {
                                worker_span.in_scope(|| {
                                    let result = run(index, signal, &stop);
                                    if failed(&result) {
                                        stop();
                                    }
                                    result
                                })
                            })
                        }));
                        if result.is_err() {
                            stop();
                        }
                        let _ = sender.send((index, result));
                    });
                    match spawned {
                        Ok(_) => running += 1,
                        Err(error) => {
                            let _state = self
                                .pool
                                .state
                                .lock()
                                .unwrap_or_else(|error| error.into_inner());
                            canceller.cancel();
                            results[index] = Some(Err(error));
                            self.pool.changed.notify_all();
                        }
                    }
                    continue;
                }
                let _ = self
                    .pool
                    .changed
                    .wait_timeout(state, Duration::from_millis(20))
                    .unwrap_or_else(|error| error.into_inner());
            }
        });
        results
    }
}

struct Permit<'a> {
    pool: &'a Pool,
}

impl Drop for Permit<'_> {
    fn drop(&mut self) {
        let mut state = self
            .pool
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.active -= 1;
        if state.active == 0 {
            state.draining = false;
        }
        self.pool.changed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;
    use std::sync::{
        Arc, Barrier,
        atomic::{AtomicBool, Ordering},
    };

    use super::*;

    #[test]
    fn failure_stops_the_batch_without_cancelling_the_attempt() {
        let executor = HookExecutor::new();
        let attempt = BuildCancellation::new();
        let epoch = executor.begin_attempt();
        executor
            .configure(
                epoch,
                true,
                HookJobs::Limit(NonZeroUsize::new(2).unwrap()),
                &attempt,
            )
            .unwrap();
        let barrier = Arc::new(Barrier::new(2));
        let stopped = AtomicBool::new(false);
        let results = executor.run_batch(
            3,
            Some(&attempt),
            |index, signal, fail| {
                barrier.wait();
                if index == 0 {
                    fail();
                    true
                } else {
                    while !signal.is_cancelled() {
                        std::thread::yield_now();
                    }
                    stopped.store(true, Ordering::Relaxed);
                    false
                }
            },
            |failed| *failed,
        );
        assert!(results[0].is_some());
        assert!(results[1].is_some());
        assert!(results[2].is_none());
        assert!(stopped.load(Ordering::Relaxed));
        assert!(!attempt.is_cancelled());
    }

    #[test]
    fn worker_panic_keeps_other_batch_results() {
        let executor = HookExecutor::new();
        let attempt = BuildCancellation::new();
        let epoch = executor.begin_attempt();
        executor
            .configure(epoch, false, HookJobs::Auto, &attempt)
            .unwrap();
        let barrier = Barrier::new(2);
        let extra_started = AtomicBool::new(false);
        let (sent, received) = std::sync::mpsc::channel();
        let results = std::thread::scope(|scope| {
            let batch = scope.spawn(|| {
                executor.run_batch(
                    3,
                    Some(&attempt),
                    |index, _, _| match index {
                        0 => {
                            sent.send(()).unwrap();
                            0
                        }
                        1 => {
                            barrier.wait();
                            panic!("worker failure");
                        }
                        _ => {
                            extra_started.store(true, Ordering::Relaxed);
                            2
                        }
                    },
                    |_| false,
                )
            });
            received.recv().unwrap();
            barrier.wait();
            batch.join().unwrap()
        });
        assert!(matches!(&results[0], Some(Ok(0))));
        assert!(matches!(&results[1], Some(Err(_))));
        assert!(results[2].is_none());
        assert!(!extra_started.load(Ordering::Relaxed));
        assert!(!attempt.is_cancelled());
    }

    #[test]
    fn invalidated_attempt_cannot_change_the_limit() {
        let executor = HookExecutor::new();
        let attempt = BuildCancellation::new();
        let epoch = executor.begin_attempt();
        executor.invalidate(epoch);
        assert!(
            executor
                .configure(epoch, false, HookJobs::Auto, &attempt)
                .is_err()
        );
    }
}
