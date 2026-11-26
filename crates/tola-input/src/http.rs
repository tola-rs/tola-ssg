//! Synchronous wait for HTTP I/O with cancellation support.

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

/// Run `req` on a shared runtime, aborting and awaiting task drop if `cancelled` is set.
pub fn wait<T: Send + 'static>(
    cancelled: &AtomicBool,
    req: impl Future<Output = T> + Send + 'static,
) -> Result<T, WaitError> {
    if cancelled.load(Ordering::Relaxed) {
        return Err(WaitError::Cancelled);
    }
    static RUNTIME: OnceLock<Result<tokio::runtime::Runtime, Arc<io::Error>>> = OnceLock::new();
    let rt = RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .build()
                .map_err(Arc::new)
        })
        .as_ref()
        .map_err(|err| WaitError::Runtime(io::Error::new(err.kind(), Arc::clone(err))))?;

    let (tx, rx) = mpsc::sync_channel(1);
    let task = rt.spawn(req);
    let abort = task.abort_handle();
    rt.spawn(async move {
        let _ = tx.send(task.await);
    });
    let res = loop {
        match rx.recv_timeout(Duration::from_millis(25)) {
            Ok(res) => break res.map_err(|_| WaitError::Stopped),
            Err(mpsc::RecvTimeoutError::Disconnected) => break Err(WaitError::Stopped),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if cancelled.load(Ordering::Relaxed) {
                    abort.abort();
                    let _ = rx.recv();
                    return Err(WaitError::Cancelled);
                }
            }
        }
    };
    if cancelled.load(Ordering::Relaxed) {
        Err(WaitError::Cancelled)
    } else {
        res
    }
}
