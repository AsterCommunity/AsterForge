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
    child.wait().await?;
    loop {
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
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
