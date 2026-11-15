//! Wait for HTTP I/O from a synchronous caller that owns a cancellation flag.

use std::future::Future;
use std::io;
use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum WaitError {
    #[error("cancelled while waiting for HTTP")]
    Cancelled,
    #[error("could not prepare the HTTP runtime")]
    Runtime(#[source] io::Error),
    #[error("HTTP task stopped before returning its response")]
    Stopped,
}

/// Run a request on a shared runtime. A cancelled wait aborts the request and waits for its
/// future to be dropped, so the request cannot retain a response body or staging file.
pub fn wait<T: Send + 'static>(
    cancelled: &AtomicBool,
    request: impl Future<Output = T> + Send + 'static,
) -> Result<T, WaitError> {
    if cancelled.load(Ordering::Relaxed) {
        return Err(WaitError::Cancelled);
    }
    static RUNTIME: OnceLock<Result<tokio::runtime::Runtime, Arc<io::Error>>> = OnceLock::new();
    let runtime = RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .build()
                .map_err(Arc::new)
        })
        .as_ref()
        .map_err(|error| WaitError::Runtime(io::Error::new(error.kind(), Arc::clone(error))))?;

    let (send, completion) = mpsc::sync_channel(1);
    let task = runtime.spawn(request);
    let abort = task.abort_handle();
    runtime.spawn(async move {
        let _ = send.send(task.await);
    });
    let result = loop {
        match completion.recv_timeout(Duration::from_millis(25)) {
            Ok(result) => break result.map_err(|_| WaitError::Stopped),
            Err(mpsc::RecvTimeoutError::Disconnected) => break Err(WaitError::Stopped),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if cancelled.load(Ordering::Relaxed) {
                    abort.abort();
                    let _ = completion.recv();
                    return Err(WaitError::Cancelled);
                }
            }
        }
    };
    if cancelled.load(Ordering::Relaxed) {
        Err(WaitError::Cancelled)
    } else {
        result
    }
}
