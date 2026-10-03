use tokio::task::JoinHandle;

/// Join handle that aborts its task if the owning boundary future is dropped.
pub(crate) struct AbortOnDrop<T> {
    handle: JoinHandle<T>,
}

impl<T> AbortOnDrop<T> {
    pub(crate) fn new(handle: JoinHandle<T>) -> Self {
        Self { handle }
    }

    pub(crate) fn handle(&mut self) -> &mut JoinHandle<T> {
        &mut self.handle
    }

    pub(crate) fn is_finished(&self) -> bool {
        self.handle.is_finished()
    }

    pub(crate) async fn abort_and_wait(&mut self) {
        self.handle.abort();
        let _ = (&mut self.handle).await;
    }
}

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.handle.abort();
    }
}
