//! Unix process-group backend.

use crate::ProcessSpec;
use process_wrap::tokio::{ChildWrapper, CommandWrap, KillOnDrop, ProcessGroup};
use std::{
    io,
    process::{ExitStatus, Stdio},
    time::Duration,
};

/// Unix process-group identity, equal to the direct child's PID.
pub(crate) type TreeIdentity = u32;

pub(crate) fn spawn(spec: &ProcessSpec) -> io::Result<super::ProcessTree> {
    let mut wrapped = CommandWrap::with_new(&spec.program, |command| {
        command
            .args(&spec.args)
            .stdin(if spec.stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if spec.clear_environment {
            command.env_clear();
        }
        command.envs(spec.environment.iter().map(|(key, value)| (key, value)));
    });
    wrapped.wrap(KillOnDrop).wrap(ProcessGroup::leader());
    let child = wrapped.spawn()?;
    let identity = child
        .id()
        .ok_or_else(|| io::Error::other("spawned child has no PID"))?;
    Ok(super::ProcessTree { child, identity })
}

pub(crate) fn start_kill(child: &mut dyn ChildWrapper) -> io::Result<()> {
    match child.start_kill() {
        Err(error) if error.raw_os_error() == Some(libc::ESRCH) => Ok(()),
        result => result,
    }
}

pub(crate) async fn wait_direct(child: &mut dyn ChildWrapper) -> io::Result<ExitStatus> {
    // SAFETY: only the native child status is observed; the process-group
    // wrapper remains owned and is used for cleanup after this wait.
    unsafe { child.try_inner_child_mut() }
        .ok_or_else(|| io::Error::other("process wrapper has no native child"))?
        .wait()
        .await
}

pub(crate) async fn wait_tree(
    child: &mut dyn ChildWrapper,
    process_id: TreeIdentity,
    direct_exited: bool,
) -> io::Result<()> {
    if !direct_exited {
        let _ = wait_direct(child).await?;
    }
    // Do not delegate to process-wrap's blocking group wait here. In a host
    // that is not a child subreaper, an adopted descendant can become a zombie
    // owned by PID 1 and remain visible to `kill(-pgid, 0)` indefinitely.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        reap_group_children(process_id)?;
        let group_id = i32::try_from(process_id)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "PID exceeds Unix range"))?;
        // SAFETY: signal 0 only probes kernel process-group membership.
        let result = unsafe { libc::kill(-group_id, 0) };
        if result == -1 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ESRCH) {
                return Ok(());
            }
            return Err(error);
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Unix process-group cleanup did not complete",
            ));
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn reap_group_children(process_id: TreeIdentity) -> io::Result<()> {
    let group_id = i32::try_from(process_id)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "PID exceeds Unix range"))?;
    loop {
        let mut status = 0;
        // SAFETY: `status` is valid for the synchronous wait, and WNOHANG
        // prevents this cleanup probe from blocking on a live descendant.
        let result = unsafe { libc::waitpid(-group_id, &raw mut status, libc::WNOHANG) };
        if result > 0 {
            continue;
        }
        if result == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ECHILD) {
            return Ok(());
        }
        return Err(error);
    }
}
