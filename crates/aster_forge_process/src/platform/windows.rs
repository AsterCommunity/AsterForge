//! Windows Job Object backend with an active-process cleanup barrier.

use crate::ProcessSpec;
use process_wrap::tokio::{ChildWrapper, CommandWrap, CommandWrapper, KillOnDrop};
use std::{
    future::Future,
    io,
    os::windows::io::{AsRawHandle, BorrowedHandle},
    pin::Pin,
    process::{ExitStatus, Stdio},
    time::Duration,
};
use tokio::process::Command;
use windows::Win32::{
    Foundation::{CloseHandle, ERROR_NO_MORE_FILES, HANDLE},
    System::{
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
        },
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation,
            QueryInformationJobObject, SetInformationJobObject, TerminateJobObject,
        },
        Threading::{CREATE_SUSPENDED, OpenThread, ResumeThread, THREAD_SUSPEND_RESUME},
    },
};

/// Windows process identity is carried by the Job Object wrapper.
#[derive(Clone, Copy)]
pub(crate) struct TreeIdentity;

/// An exclusively owned kernel handle, closed on every exit path.
#[derive(Debug)]
struct OwnedHandle(HANDLE);

// SAFETY: these Job Object, thread and snapshot handles are not thread-affine;
// ownership moves with the wrapper and Windows synchronizes kernel access.
unsafe impl Send for OwnedHandle {}
// SAFETY: shared references only perform kernel queries; closing requires the
// exclusive ownership held by Drop, and cannot overlap a borrowed query.
unsafe impl Sync for OwnedHandle {}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: this handle is owned here and closed exactly once.
        unsafe { CloseHandle(self.0) }.ok();
    }
}

#[derive(Debug)]
/// Suspend before spawning so assignment to the Job precedes child execution.
pub(crate) struct JobObject;

impl CommandWrapper for JobObject {
    fn pre_spawn(&mut self, command: &mut Command, _core: &CommandWrap) -> io::Result<()> {
        command.creation_flags(CREATE_SUSPENDED.0);
        Ok(())
    }

    fn wrap_child(
        &mut self,
        child: Box<dyn ChildWrapper>,
        _core: &CommandWrap,
    ) -> io::Result<Box<dyn ChildWrapper>> {
        let process = child
            .process_handle()
            .map(|h| HANDLE(h.as_raw_handle()))
            .ok_or_else(|| io::Error::other("child process handle unavailable"))?;
        // SAFETY: no name or security attributes are supplied; ownership is transferred to OwnedHandle.
        let job = OwnedHandle(unsafe { CreateJobObjectW(None, None) }.map_err(io::Error::other)?);
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let limits_size =
            u32::try_from(std::mem::size_of_val(&limits)).map_err(io::Error::other)?;
        // SAFETY: limits is initialized and passed with its exact size.
        unsafe {
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast(),
                limits_size,
            )
        }
        .map_err(io::Error::other)?;
        // SAFETY: both process and job handles are live for this synchronous call.
        unsafe { AssignProcessToJobObject(job.0, process) }.map_err(io::Error::other)?;
        let pid = child
            .id()
            .ok_or_else(|| io::Error::other("child process ID unavailable"))?;
        resume_process(pid)?;
        Ok(Box::new(JobChild { child, job }))
    }
}

#[derive(Debug)]
/// Keep the Job alive until both direct-child reaping and the cleanup barrier finish.
struct JobChild {
    child: Box<dyn ChildWrapper>,
    job: OwnedHandle,
}

impl ChildWrapper for JobChild {
    fn inner(&self) -> &dyn ChildWrapper {
        self.child.as_ref()
    }
    fn inner_mut(&mut self) -> &mut dyn ChildWrapper {
        self.child.as_mut()
    }
    fn into_inner(self: Box<Self>) -> Box<dyn ChildWrapper> {
        self.child
    }
    fn process_handle(&self) -> Option<BorrowedHandle<'_>> {
        self.child.process_handle()
    }
    fn start_kill(&mut self) -> io::Result<()> {
        // SAFETY: job remains owned and valid until this wrapper is dropped.
        unsafe { TerminateJobObject(self.job.0, 1) }.map_err(io::Error::other)
    }
    fn wait(&mut self) -> Pin<Box<dyn Future<Output = io::Result<ExitStatus>> + Send + '_>> {
        let job = &self.job;
        let child = &mut self.child;
        Box::pin(async move {
            let status = child.wait().await?;
            wait_job_empty(job).await?;
            Ok(status)
        })
    }
}

/// Spawn into a kill-on-close Job before resuming the suspended child.
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
    wrapped.wrap(KillOnDrop).wrap(JobObject);
    let child = wrapped.spawn()?;
    Ok(super::ProcessTree {
        child,
        identity: TreeIdentity,
    })
}

/// Request termination of every process assigned to the Job.
pub(crate) fn start_kill(child: &mut dyn ChildWrapper) -> io::Result<()> {
    child.start_kill()
}
/// Observe direct-child status without waiting on descendants or output EOF.
pub(crate) async fn wait_direct(child: &mut dyn ChildWrapper) -> io::Result<ExitStatus> {
    // SAFETY: only the native child is waited; the wrapper retains ownership of
    // its Job and the separate tree wait still enforces the cleanup barrier.
    unsafe { child.try_inner_child_mut() }
        .ok_or_else(|| io::Error::other("native child unavailable"))?
        .wait()
        .await
}
/// Reap the direct child before waiting until the Job has no active processes.
pub(crate) async fn wait_tree(
    child: &mut dyn ChildWrapper,
    _identity: TreeIdentity,
    direct_exited: bool,
) -> io::Result<()> {
    if !direct_exited {
        let _ = wait_direct(child).await?;
    }
    child.wait().await.map(|_| ())
}

/// Query kernel Job membership, independent of completion-port notifications
/// and whether any ancestor PID is still present in a process snapshot.
async fn wait_job_empty(job: &OwnedHandle) -> io::Result<()> {
    loop {
        let mut accounting = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
        let accounting_size =
            u32::try_from(std::mem::size_of_val(&accounting)).map_err(io::Error::other)?;
        // SAFETY: the borrowed Job remains live across this query; accounting
        // is initialized, exclusively writable, and passed with its exact size.
        unsafe {
            QueryInformationJobObject(
                Some(job.0),
                JobObjectBasicAccountingInformation,
                (&raw mut accounting).cast(),
                accounting_size,
                None,
            )
        }
        .map_err(io::Error::other)?;
        if accounting.ActiveProcesses == 0 {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Resume the child's initial thread after Job assignment. All snapshot and
/// thread handles close through RAII, including enumeration/resume failures.
fn resume_process(pid: u32) -> io::Result<()> {
    // SAFETY: flags request a system thread snapshot; its handle is owned locally.
    let snapshot = OwnedHandle(
        unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) }.map_err(io::Error::other)?,
    );
    let mut entry = THREADENTRY32 {
        dwSize: u32::try_from(std::mem::size_of::<THREADENTRY32>()).map_err(io::Error::other)?,
        ..Default::default()
    };
    // SAFETY: entry is initialized and exclusively borrowed by the synchronous API.
    unsafe { Thread32First(snapshot.0, &raw mut entry) }.map_err(io::Error::other)?;
    let mut resumed = false;
    loop {
        if entry.th32OwnerProcessID == pid {
            // SAFETY: thread ID came from the live snapshot; handle is closed below.
            let thread = OwnedHandle(
                unsafe { OpenThread(THREAD_SUSPEND_RESUME, false, entry.th32ThreadID) }
                    .map_err(io::Error::other)?,
            );
            // SAFETY: thread has suspend/resume rights.
            let result = unsafe { ResumeThread(thread.0) };
            if result == u32::MAX {
                return Err(io::Error::last_os_error());
            }
            resumed = true;
        }
        // SAFETY: entry remains exclusively borrowed and snapshot remains valid.
        if let Err(error) = unsafe { Thread32Next(snapshot.0, &raw mut entry) } {
            if error.code() == ERROR_NO_MORE_FILES.to_hresult() {
                break;
            }
            return Err(io::Error::other(error));
        }
    }
    resumed
        .then_some(())
        .ok_or_else(|| io::Error::other("suspended process thread unavailable"))
}
