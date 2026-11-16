//! Platform-specific process containment and cleanup backends.

use crate::ProcessSpec;
use process_wrap::tokio::ChildWrapper;
use std::{io, process::ExitStatus};

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
use unix as backend;
#[cfg(windows)]
use windows as backend;

/// A spawned child together with its platform ownership handle.
pub(crate) struct ProcessTree {
    pub(crate) child: Box<dyn ChildWrapper>,
    pub(crate) identity: backend::TreeIdentity,
}

impl ProcessTree {
    /// Spawn a directly executed child in the platform containment primitive.
    pub(crate) fn spawn(spec: &ProcessSpec) -> io::Result<Self> {
        backend::spawn(spec)
    }

    pub(crate) fn start_kill(&mut self) -> io::Result<()> {
        backend::start_kill(&mut *self.child)
    }

    pub(crate) async fn wait_direct(&mut self) -> io::Result<ExitStatus> {
        backend::wait_direct(&mut *self.child).await
    }

    pub(crate) async fn wait_tree(&mut self, direct_exited: bool) -> io::Result<()> {
        backend::wait_tree(&mut *self.child, self.identity, direct_exited).await
    }
}

impl Drop for ProcessTree {
    fn drop(&mut self) {
        let _ = self.start_kill();
    }
}
