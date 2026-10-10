//! Owner-bound native lookup work. Cancellation aborts queued jobs and signals
//! running batches to stop admitting further lookups after their current call.
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tokio::task::{JoinError, JoinHandle};

struct OwnedJob<T> {
    handle: JoinHandle<T>,
    cancelled: Arc<AtomicBool>,
}
impl<T> Drop for OwnedJob<T> {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        self.handle.abort();
    }
}

pub(crate) async fn run<T: Send + 'static>(
    operation: impl FnOnce(&AtomicBool) -> T + Send + 'static,
) -> Result<T, JoinError> {
    let cancelled = Arc::new(AtomicBool::new(false));
    let worker_cancelled = cancelled.clone();
    let mut job = OwnedJob {
        handle: tokio::task::spawn_blocking(move || operation(&worker_cancelled)),
        cancelled,
    };
    (&mut job.handle).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::{Condvar, Mutex},
        time::Duration,
    };
    use tokio::sync::Notify;

    #[derive(Default)]
    struct Gate {
        open: Mutex<bool>,
        signal: Condvar,
    }
    impl Gate {
        fn wait(&self) {
            let mut open = self.open.lock().unwrap();
            while !*open {
                open = self.signal.wait(open).unwrap();
            }
        }
        fn release(&self) {
            *self.open.lock().unwrap() = true;
            self.signal.notify_all();
        }
    }
    struct Release(Arc<Gate>);
    impl Drop for Release {
        fn drop(&mut self) {
            self.0.release();
        }
    }

    #[test]
    fn cancellation_and_deadline_abort_queued_jobs_before_they_run() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(1)
            .build()
            .unwrap();
        runtime.block_on(async {
            for timeout in [false, true] {
                let gate = Arc::new(Gate::default());
                let _release = Release(gate.clone());
                let started = Arc::new(Notify::new());
                let blocker_gate = gate.clone();
                let blocker_started = started.clone();
                let blocker = tokio::task::spawn_blocking(move || {
                    blocker_started.notify_one();
                    blocker_gate.wait();
                });
                started.notified().await;
                let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
                let lifetime = Arc::new(());
                let worker_lifetime = lifetime.clone();
                let worker_calls = calls.clone();
                let queued = Arc::new(Notify::new());
                let owner_queued = queued.clone();
                let owner = tokio::spawn(async move {
                    owner_queued.notify_one();
                    tokio::time::timeout(
                        Duration::from_secs(5),
                        run(move |_| {
                            worker_calls.fetch_add(1, Ordering::SeqCst);
                            drop(worker_lifetime);
                        }),
                    )
                    .await
                });
                queued.notified().await;
                if timeout {
                    tokio::time::pause();
                    tokio::time::advance(Duration::from_secs(6)).await;
                    tokio::time::resume();
                    assert!(owner.await.unwrap().is_err());
                } else {
                    owner.abort();
                    assert!(owner.await.unwrap_err().is_cancelled());
                }
                gate.release();
                blocker.await.unwrap();
                tokio::time::timeout(Duration::from_secs(3), async {
                    while Arc::strong_count(&lifetime) != 1 {
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .unwrap();
                assert_eq!(calls.load(Ordering::SeqCst), 0);
            }
        });
    }

    #[test]
    fn started_batch_finishes_current_call_and_observes_cancellation() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let gate = Arc::new(Gate::default());
            let _release = Release(gate.clone());
            let started = Arc::new(Notify::new());
            let worker_started = started.clone();
            let worker_gate = gate.clone();
            let continued = Arc::new(AtomicBool::new(false));
            let worker_continued = continued.clone();
            let lifetime = Arc::new(());
            let worker_lifetime = lifetime.clone();
            let owner = tokio::spawn(run(move |cancelled| {
                worker_started.notify_one();
                worker_gate.wait();
                worker_continued.store(!cancelled.load(Ordering::Acquire), Ordering::Release);
                drop(worker_lifetime);
            }));
            started.notified().await;
            owner.abort();
            assert!(owner.await.unwrap_err().is_cancelled());
            gate.release();
            tokio::time::timeout(Duration::from_secs(3), async {
                while Arc::strong_count(&lifetime) != 1 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            assert!(!continued.load(Ordering::Acquire));
        });
    }
}
