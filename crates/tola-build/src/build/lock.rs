//! Cross-process exclusion for one site's source generation and build attempt.

use std::sync::Arc;

use anyhow::Result;
use thiserror::Error;

use super::{BuildFailure, BuildRequest, BuildSession, SiteBuild};
use crate::cancellation::BuildCancellation;
use crate::config::ResolvedSiteConfig;

/// Why this site's build lock could not be taken.
#[derive(Debug, Error)]
pub enum SiteBuildLockError {
    /// The waiter stopped before the lock became available.
    #[error(transparent)]
    Cancelled(#[from] crate::cancellation::BuildCancelled),

    #[error(
        "the site directory could not be used: {reason}; run the command from the site directory or pass `--config`"
    )]
    SiteDirectory {
        reason: &'static str,
        #[source]
        source: std::io::Error,
    },

    #[error(
        "`{lock}` is not a regular file, so it cannot hold this site's build lock; stop other Tola commands and move that path aside"
    )]
    LockPath { lock: String },

    #[error(
        "could not open the site's build lock `{lock}`: {reason}; make the site directory writable"
    )]
    LockAccess {
        lock: String,
        reason: &'static str,
        #[source]
        source: std::io::Error,
    },
}

/// Excludes another coordinated build of the same site until this value is dropped.
///
/// Acquire on a blocking worker before source generation; hold through publication
/// or revision installation, not filesystem-event waits or HTTP serving.
#[derive(Debug)]
pub struct SiteBuildLock {
    _lock: crate::filesystem::FileLock,
    site_root: std::path::PathBuf,
}

impl SiteBuildLock {
    /// Wait for the site's build lock, checking cancellation between attempts.
    /// `on_wait` is called once, only if another build holds the lock.
    /// Filesystem errors are returned immediately rather than treated as contention.
    pub fn acquire(
        config: &ResolvedSiteConfig,
        cancellation: &BuildCancellation,
        on_wait: impl FnOnce(),
    ) -> Result<Self, SiteBuildLockError> {
        Self::acquire_at_root(config.get_root(), cancellation, on_wait)
    }

    pub(crate) fn acquire_at_root(
        site_root: &std::path::Path,
        cancellation: &BuildCancellation,
        on_wait: impl FnOnce(),
    ) -> Result<Self, SiteBuildLockError> {
        cancellation.ensure_active()?;
        let site_root = std::fs::canonicalize(site_root).map_err(|source| {
            SiteBuildLockError::SiteDirectory {
                reason: crate::filesystem::path_failure_reason(&source),
                source,
            }
        })?;
        let path = site_root.join(crate::filesystem::SITE_BUILD_LOCK_FILE);
        let lock = crate::filesystem::FileLock::acquire(&site_root, &path, cancellation, on_wait)
            .map_err(|error| match error {
            crate::filesystem::FileLockError::Cancelled(cancelled) => {
                SiteBuildLockError::Cancelled(cancelled)
            }
            crate::filesystem::FileLockError::NotRegularFile => SiteBuildLockError::LockPath {
                lock: crate::filesystem::display_path(&path, &site_root),
            },
            crate::filesystem::FileLockError::Io(error) => SiteBuildLockError::LockAccess {
                lock: crate::filesystem::display_path(&path, &site_root),
                reason: crate::filesystem::path_failure_reason(&error),
                source: error,
            },
        })?;
        Ok(Self {
            _lock: lock,
            site_root,
        })
    }

    pub(crate) fn guards(&self, site_root: &std::path::Path) -> bool {
        self.site_root == site_root
    }

    /// Recover before source generation when this attempt intends to publish on disk.
    /// Acquiring the lock alone never changes the published tree.
    pub fn recover_output(
        &self,
        config: &ResolvedSiteConfig,
        cancellation: &BuildCancellation,
    ) -> Result<()> {
        let recover = || -> Result<()> {
            let destination =
                crate::output::resolve_site_output_root_with_inputs(config, std::iter::empty())?;
            destination.write(self, cancellation)?;
            Ok(())
        };
        recover().map_err(|error| super::diagnostic::with_write(error, config.get_root()))
    }

    /// Publish using this held authority; consumers must run after releasing it.
    pub fn write_site(&self, build: &SiteBuild) -> Result<super::WriteOutcome> {
        super::revision::write_site_locked(build, self)
    }
}

/// A completed build that keeps the site's build lock held.
///
/// The held lock orders source generation, the attempt, and any publication or
/// revision installation against every other coordinated build of the same site.
/// Drop this value to release the lock, or use [`Self::release`] to keep the build.
pub struct SiteBuildGuard {
    build: SiteBuild,
    lock: SiteBuildLock,
}

impl SiteBuildGuard {
    /// Run one coordinated attempt, waiting for the site's build lock.
    ///
    /// `on_wait` runs once, and only if another coordinated build holds the lock.
    pub fn build(
        session: &mut BuildSession,
        config: Arc<ResolvedSiteConfig>,
        request: BuildRequest,
        on_wait: impl FnOnce(),
    ) -> Result<Self, BuildFailure> {
        Self::lock_and_build(session, config, request, on_wait, false)
    }

    /// Restore interrupted output before compiling a disk-publishing attempt.
    pub fn build_for_publication(
        session: &mut BuildSession,
        config: Arc<ResolvedSiteConfig>,
        request: BuildRequest,
        on_wait: impl FnOnce(),
    ) -> Result<Self, BuildFailure> {
        Self::lock_and_build(session, config, request, on_wait, true)
    }

    /// Hold the site lock through one attempt, recovering interrupted output first when writing.
    fn lock_and_build(
        session: &mut BuildSession,
        config: Arc<ResolvedSiteConfig>,
        request: BuildRequest,
        on_wait: impl FnOnce(),
        recover_output: bool,
    ) -> Result<Self, BuildFailure> {
        let lock = SiteBuildLock::acquire(&config, &request.cancellation, on_wait)
            .map_err(|error| BuildFailure::before_attempt(anyhow::Error::new(error)))?;
        if recover_output {
            lock.recover_output(&config, &request.cancellation)
                .map_err(BuildFailure::before_attempt)?;
        }
        let build = session.prepare(config, request).run()?;
        Ok(Self { build, lock })
    }

    /// Borrow this attempt's site lock rather than acquiring it recursively.
    pub fn write_site(&self) -> Result<super::WriteOutcome> {
        self.lock.write_site(&self.build)
    }

    #[inline]
    pub fn site(&self) -> &SiteBuild {
        &self.build
    }

    /// Verify that the build still matches the inputs it was built from.
    pub fn ensure_fresh(
        &self,
        cancellation: &crate::cancellation::BuildCancellation,
    ) -> anyhow::Result<()> {
        self.build.ensure_fresh(cancellation)
    }

    /// Release the site build lock, keeping the built site.
    pub fn release(self) -> SiteBuild {
        self.build
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::tests::load_test_config;
    use crate::mode::BuildMode;
    use crate::resources::BuildResources;

    #[test]
    fn cancelled_wait_leaves_the_lock_held() {
        let directory = tempfile::tempdir().unwrap();
        let config = crate::config::tests::load_test_config(directory.path(), "");
        let owner = SiteBuildLock::acquire(&config, &BuildCancellation::default(), || {
            panic!("uncontended build waited")
        })
        .unwrap();
        let canceller = crate::cancellation::BuildCanceller::default();
        let blocked = SiteBuildLock::acquire(&config, &canceller.token(), || canceller.cancel());
        assert!(matches!(
            blocked.unwrap_err(),
            SiteBuildLockError::Cancelled(_)
        ));
        drop(owner);
        SiteBuildLock::acquire(&config, &BuildCancellation::default(), || {
            panic!("released build lock remained held")
        })
        .unwrap();
    }

    #[test]
    fn live_attempt_holds_the_build_lock() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir(directory.path().join("content")).unwrap();
        std::fs::write(directory.path().join("site.typ"), "").unwrap();
        let config = Arc::new(load_test_config(directory.path(), ""));
        let holding = SiteBuildGuard::build(
            &mut BuildSession::with_resources(BuildResources::default()),
            Arc::clone(&config),
            BuildRequest::new(BuildMode::Production),
            || panic!("uncontended build waited"),
        )
        .map_err(BuildFailure::into_error)
        .unwrap();

        let (started, waited) = std::sync::mpsc::channel();
        let waiting = std::thread::scope(|scope| {
            let waiter = scope.spawn(|| {
                let mut session = BuildSession::with_resources(BuildResources::default());
                SiteBuildGuard::build(
                    &mut session,
                    Arc::clone(&config),
                    BuildRequest::new(BuildMode::Production),
                    || started.send(()).unwrap(),
                )
            });
            waited.recv().unwrap();
            if waiter.is_finished() {
                panic!("waiter acquired a build lock that a live locked attempt holds");
            }
            drop(holding);
            waiter.join().unwrap()
        });
        assert!(waiting.is_ok(), "released lock was not acquirable");
    }
}
