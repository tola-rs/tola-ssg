//! One started child: the wrapper that contains it, its termination, and what a
//! finished run reports.

use std::process::ExitStatus;

use process_wrap::std::ChildWrapper;

use crate::{Captured, Error, Result, sys};

/// How a command stopped.
#[derive(Debug)]
#[non_exhaustive]
pub enum Stop {
    /// The command exited on its own.
    Exited(ExitStatus),
    /// The caller's cancellation was observed and the tree was terminated.
    ///
    /// The status is present when the terminated child's exit was observable.
    Cancelled(Option<ExitStatus>),
}

impl Stop {
    /// The observed exit status, when there was one.
    pub fn status(&self) -> Option<ExitStatus> {
        match self {
            Self::Exited(status) => Some(*status),
            Self::Cancelled(status) => *status,
        }
    }

    pub fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled(_))
    }
}

/// Everything one finished command leaves behind.
#[derive(Debug)]
pub struct Exit {
    pub stop: Stop,
    pub stdout: Captured,
    pub stderr: Captured,
}

impl Exit {
    /// A run that stopped before it could start: cancellation was observed first.
    pub(crate) fn cancelled(status: Option<ExitStatus>) -> Self {
        Self {
            stop: Stop::Cancelled(status),
            stdout: Captured::default(),
            stderr: Captured::default(),
        }
    }
}

/// How far this child's observation and termination have gone.
///
/// `Terminated` is recorded even when the kill failed, so termination is attempted once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChildLifecycle {
    Running,
    Exited(ExitStatus),
    Terminated(Option<ExitStatus>),
}

/// Owns termination even when initialization or a caller's callback unwinds.
pub(crate) struct Child {
    wrapper: Box<dyn ChildWrapper>,
    lifecycle: ChildLifecycle,
}

impl Child {
    pub(crate) fn new(wrapper: Box<dyn ChildWrapper>) -> Self {
        Self {
            wrapper,
            lifecycle: ChildLifecycle::Running,
        }
    }

    /// The wrapper that owns this child's containment domain.
    pub(crate) fn wrapper_mut(&mut self) -> &mut dyn ChildWrapper {
        self.wrapper.as_mut()
    }

    /// Observe the direct child's exit without blocking. The inner child caches
    /// the status it reaped, so later calls report the same one.
    pub(crate) fn try_wait(&mut self) -> Result<Option<ExitStatus>> {
        if let ChildLifecycle::Terminated(status) = self.lifecycle {
            return Ok(status);
        }
        let status = self.wrapper.inner_mut().try_wait().map_err(Error::Wait)?;
        if let Some(status) = status {
            self.lifecycle = ChildLifecycle::Exited(status);
        }
        Ok(status)
    }

    /// Terminate the direct child and the descendants that stayed with it.
    pub(crate) fn terminate(&mut self) -> Option<ExitStatus> {
        let status = match self.lifecycle {
            ChildLifecycle::Running => sys::terminate(self.wrapper.as_mut()),
            ChildLifecycle::Exited(status) => {
                let _ = self.wrapper.start_kill();
                Some(status)
            }
            ChildLifecycle::Terminated(status) => return status,
        };
        self.lifecycle = ChildLifecycle::Terminated(status);
        status
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        self.terminate();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    #[test]
    fn terminated_child_keeps_observed_status() {
        let command = Command::new(std::env::current_exe().unwrap())
            .arg("--list")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut child = Child::new(Box::new(command));

        let status = child.terminate().expect("the terminated child was reaped");

        assert_eq!(child.try_wait().unwrap(), Some(status));
        assert_eq!(child.terminate(), Some(status));
    }
}
