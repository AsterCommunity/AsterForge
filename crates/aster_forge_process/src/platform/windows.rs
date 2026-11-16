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
    Foundation::{CloseHandle, HANDLE},
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
pub(crate) type TreeIdentity = ();

#[derive(Debug)]
struct OwnedJob(HANDLE);

// SAFETY: the OS synchronizes access to a Job Object handle.
unsafe impl Send for OwnedJob {}
// SAFETY: the OS synchronizes access to a Job Object handle.
unsafe impl Sync for OwnedJob {}

impl Drop for OwnedJob {
    fn drop(&mut self) {
        // SAFETY: this handle is owned here and closed exactly once.
        unsafe { CloseHandle(self.0) }.ok();
    }
}

#[derive(Debug)]
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
        // SAFETY: no name or security attributes are supplied; ownership is transferred to OwnedJob.
        let job = OwnedJob(unsafe { CreateJobObjectW(None, None) }.map_err(io::Error::other)?);
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: limits is initialized and passed with its exact size.
        unsafe {
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of_val(&limits) as u32,
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
struct JobChild {
    child: Box<dyn ChildWrapper>,
    job: OwnedJob,
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
        let job = self.job.0.0 as usize;
        let child = &mut self.child;
        Box::pin(async move {
            let status = child.wait().await?;
            wait_job_empty(job as isize).await?;
            Ok(status)
        })
    }
}

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
        identity: (),
    })
}

pub(crate) fn start_kill(child: &mut dyn ChildWrapper) -> io::Result<()> {
    child.start_kill()
}
pub(crate) async fn wait_direct(child: &mut dyn ChildWrapper) -> io::Result<ExitStatus> {
    unsafe { child.try_inner_child_mut() }
        .ok_or_else(|| io::Error::other("native child unavailable"))?
        .wait()
        .await
}
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

async fn wait_job_empty(job: isize) -> io::Result<()> {
    loop {
        let mut accounting = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
        // SAFETY: accounting is exclusively initialized for this synchronous query.
        unsafe {
            QueryInformationJobObject(
                Some(HANDLE(job as *mut std::ffi::c_void)),
                JobObjectBasicAccountingInformation,
                (&mut accounting as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                std::mem::size_of_val(&accounting) as u32,
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

fn resume_process(pid: u32) -> io::Result<()> {
    // SAFETY: snapshot is owned locally and closed before return.
    let snapshot =
        unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) }.map_err(io::Error::other)?;
    let mut entry = THREADENTRY32 {
        dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
        ..Default::default()
    };
    // SAFETY: entry is initialized and exclusively borrowed by the synchronous API.
    unsafe { Thread32First(snapshot, &mut entry) }.map_err(io::Error::other)?;
    let mut resumed = false;
    loop {
        if entry.th32OwnerProcessID == pid {
            // SAFETY: thread ID came from the live snapshot; handle is closed below.
            let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, false, entry.th32ThreadID) }
                .map_err(io::Error::other)?;
            // SAFETY: thread has suspend/resume rights.
            let result = unsafe { ResumeThread(thread) };
            // SAFETY: thread is a live handle owned by this scope.
            unsafe { CloseHandle(thread) }.ok();
            if result == u32::MAX {
                unsafe { CloseHandle(snapshot) }.ok();
                return Err(io::Error::last_os_error());
            }
            resumed = true;
        }
        // SAFETY: entry remains exclusively borrowed and snapshot remains valid.
        if unsafe { Thread32Next(snapshot, &mut entry) }.is_err() {
            break;
        }
    }
    // SAFETY: snapshot is still valid and owned exactly once.
    unsafe { CloseHandle(snapshot) }.ok();
    resumed
        .then_some(())
        .ok_or_else(|| io::Error::other("suspended process thread unavailable"))
}
