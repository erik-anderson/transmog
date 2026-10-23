//! Bound native test concurrency while retaining the production worker quotas.

use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{Semaphore, SemaphorePermit};

// Each ordinary case uses at most three live workers, below the global limit of
// sixteen even with four cases running. Quota and slot-release cases acquire all
// permits, so other tests cannot affect their counts. Each test binary has its
// own pool, matching the supervisor's process-local accounting.
pub const PARALLEL_TESTS: u32 = 4;
static TESTS: Semaphore = Semaphore::const_new(PARALLEL_TESTS as usize);
static NEXT_PACKAGE: AtomicU64 = AtomicU64::new(0);

pub struct WorkerTest {
    _permit: SemaphorePermit<'static>,
    package: String,
}

impl WorkerTest {
    // Declare this guard before workers/registries, so their cleanup finishes
    // before another case can acquire its permits.
    pub async fn acquire(permits: u32) -> Self {
        assert!((1..=PARALLEL_TESTS).contains(&permits));
        let permit = TESTS.acquire_many(permits).await.unwrap();
        Self {
            _permit: permit,
            package: format!(
                "example.test-{}",
                NEXT_PACKAGE.fetch_add(1, Ordering::Relaxed)
            ),
        }
    }

    pub fn package(&self) -> &str {
        &self.package
    }
}
