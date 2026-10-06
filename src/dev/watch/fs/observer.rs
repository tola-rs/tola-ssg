//! Native watcher creation, updates, and destruction on one blocking thread.
//!
//! Updates must be awaited outside revision replacement locks. Watching ends when the control
//! channel closes, not when a build ends.

use notify::{PathOp, UpdatePathsError, Watcher};
use tokio::sync::{mpsc, oneshot};

struct DirectoryChange {
    operations: Vec<PathOp>,
    completed: oneshot::Sender<Result<(), UpdatePathsError>>,
}

pub(super) struct DirectoryObserver {
    changes: mpsc::Sender<DirectoryChange>,
    worker: std::thread::JoinHandle<()>,
}

impl DirectoryObserver {
    pub(super) async fn start<W: Watcher + 'static>(
        create: impl FnOnce() -> notify::Result<W> + Send + 'static,
    ) -> notify::Result<Self> {
        let (changes, mut pending) = mpsc::channel::<DirectoryChange>(1);
        let (started, startup) = oneshot::channel();
        let worker = std::thread::Builder::new()
            .name("tola filesystem observation".into())
            .spawn(move || {
                let mut watcher = match create() {
                    Ok(watcher) => watcher,
                    Err(error) => {
                        let _ = started.send(Err(error));
                        return;
                    }
                };
                if started.send(Ok(())).is_err() {
                    return;
                }
                while let Some(change) = pending.blocking_recv() {
                    let applied = watcher.update_paths(change.operations);
                    let _ = change.completed.send(applied);
                }
            })
            .map_err(notify::Error::io)?;
        let observer = Self { changes, worker };
        match startup.await {
            Ok(Ok(())) => Ok(observer),
            startup => {
                observer.shutdown().await?;
                Err(match startup {
                    Ok(Err(error)) => error,
                    Err(_) => notify::Error::generic("filesystem observation ended during startup"),
                    Ok(Ok(())) => unreachable!(),
                })
            }
        }
    }

    pub(super) async fn update(&mut self, operations: Vec<PathOp>) -> Result<(), UpdatePathsError> {
        let (completed, applied) = oneshot::channel();
        self.changes
            .send(DirectoryChange {
                operations,
                completed,
            })
            .await
            .map_err(|error| UpdatePathsError {
                source: notify::Error::generic(
                    "filesystem observation stopped before updating directories",
                ),
                origin: None,
                remaining: error.0.operations,
            })?;
        applied.await.unwrap_or_else(|_| {
            Err(UpdatePathsError {
                source: notify::Error::generic(
                    "filesystem observation stopped while updating directories",
                ),
                origin: None,
                remaining: Vec::new(),
            })
        })
    }

    pub(super) async fn shutdown(self) -> notify::Result<()> {
        let Self { changes, worker } = self;
        drop(changes);
        tokio::task::spawn_blocking(move || worker.join())
            .await
            .map_err(|error| {
                notify::Error::generic(&format!("filesystem observation join failed: {error}"))
            })?
            .map_err(|_| notify::Error::generic("filesystem observation thread panicked"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    struct ThreadBoundWatcher {
        owner: std::thread::ThreadId,
        watched: Vec<PathBuf>,
        closed: Option<oneshot::Sender<Vec<PathBuf>>>,
    }

    impl Watcher for ThreadBoundWatcher {
        fn new<F: notify::EventHandler>(_: F, _: notify::Config) -> notify::Result<Self> {
            Err(notify::Error::generic(
                "the thread-bound watcher requires an observation factory",
            ))
        }

        fn watch(&mut self, path: &Path, _: notify::RecursiveMode) -> notify::Result<()> {
            assert_eq!(std::thread::current().id(), self.owner);
            if path == Path::new("missing") {
                return Err(notify::Error::path_not_found().add_path(path.to_path_buf()));
            }
            self.watched.push(path.to_path_buf());
            Ok(())
        }

        fn unwatch(&mut self, path: &Path) -> notify::Result<()> {
            assert_eq!(std::thread::current().id(), self.owner);
            self.watched.retain(|watched| watched != path);
            Ok(())
        }

        fn kind() -> notify::WatcherKind {
            notify::WatcherKind::NullWatcher
        }
    }

    impl Drop for ThreadBoundWatcher {
        fn drop(&mut self) {
            assert_eq!(std::thread::current().id(), self.owner);
            let _ = self
                .closed
                .take()
                .unwrap()
                .send(std::mem::take(&mut self.watched));
        }
    }

    #[tokio::test]
    async fn watcher_updates_stay_on_thread() {
        let caller = std::thread::current().id();
        let (closed, observed) = oneshot::channel();
        let mut observer = DirectoryObserver::start(move || {
            let owner = std::thread::current().id();
            assert_ne!(owner, caller);
            Ok(ThreadBoundWatcher {
                owner,
                watched: Vec::new(),
                closed: Some(closed),
            })
        })
        .await
        .unwrap();
        observer
            .update(vec![PathOp::watch_recursive("content")])
            .await
            .unwrap();
        observer
            .update(vec![
                PathOp::watch_recursive("templates"),
                PathOp::unwatch("content"),
            ])
            .await
            .unwrap();
        observer.shutdown().await.unwrap();
        assert_eq!(observed.await.unwrap(), [PathBuf::from("templates")]);
    }

    #[tokio::test]
    async fn failed_update_reports_unapplied_paths() {
        let (closed, observed) = oneshot::channel();
        let mut observer = DirectoryObserver::start(move || {
            Ok(ThreadBoundWatcher {
                owner: std::thread::current().id(),
                watched: Vec::new(),
                closed: Some(closed),
            })
        })
        .await
        .unwrap();
        let failure = observer
            .update(vec![
                PathOp::watch_recursive("content"),
                PathOp::watch_recursive("missing"),
                PathOp::watch_recursive("templates"),
            ])
            .await
            .unwrap_err();
        assert!(matches!(
            failure.source.kind,
            notify::ErrorKind::PathNotFound
        ));
        assert!(
            matches!(failure.origin, Some(PathOp::Watch(path, _)) if path == Path::new("missing"))
        );
        assert!(
            matches!(failure.remaining.as_slice(), [PathOp::Watch(path, _)] if path == Path::new("templates"))
        );
        observer.shutdown().await.unwrap();
        assert_eq!(observed.await.unwrap(), [PathBuf::from("content")]);
    }
}
