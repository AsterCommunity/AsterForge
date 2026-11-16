//! Boundary tests use a real child process so pipe, deadline and tree ownership
//! behavior cannot be hidden behind mocks.
use aster_forge_process::{ProcessFailure, ProcessSpec, ProcessStream, run};
use std::{
    io::{Read as _, Write as _},
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio_util::sync::CancellationToken;

fn main() {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() == Some("--fixture") {
        child(args.next().as_deref().unwrap_or_default());
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(check());
    println!("aster_forge_process boundary contract passed");
}

#[expect(
    clippy::zombie_processes,
    reason = "The fixture intentionally leaves descendants for the process-tree contract to clean up."
)]
fn child(mode: &str) {
    match mode {
        "closed" => (),
        "sleep" => std::thread::sleep(Duration::from_secs(30)),
        #[cfg(unix)]
        "close-stdin-sleep" => {
            // SAFETY: this runs only in the isolated fixture subprocess and closes its inherited stdin pipe.
            unsafe { libc::close(libc::STDIN_FILENO) };
            std::thread::sleep(Duration::from_secs(30));
        }
        "env" => {
            assert!(std::env::var_os("PATH").is_none());
            print!(
                "{}",
                std::env::var("PROCESS_FIXTURE_VALUE").unwrap_or_default()
            );
        }
        "inherited-env" => {
            print!("{}", usize::from(std::env::var_os("PATH").is_some()));
        }
        "tree" => {
            let child = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--fixture", "sleep"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            println!("{}", child.id());
            std::io::stdout().flush().unwrap();
            std::thread::sleep(Duration::from_secs(30));
        }
        "orphan" => {
            let child = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--fixture", "sleep"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            println!("{}", child.id());
            std::io::stdout().flush().unwrap();
        }
        "streams" => {
            std::io::stdout()
                .write_all(&vec![b'o'; 128 * 1024])
                .unwrap();
            std::io::stderr()
                .write_all(&vec![b'e'; 128 * 1024])
                .unwrap();
        }
        _ => {
            assert!(std::env::var_os("PATH").is_none());
            let mut input = Vec::new();
            std::io::stdin().read_to_end(&mut input).unwrap();
            std::io::stdout().write_all(&input).unwrap();
        }
    }
}

fn spec(mode: &str) -> ProcessSpec {
    let mut spec = ProcessSpec::new(std::env::current_exe().unwrap(), Duration::from_secs(5));
    spec.args = vec!["--fixture".into(), mode.into()];
    spec.clear_environment = true;
    // Preserve instrumented subprocess coverage while keeping generated data under Cargo's ignored target dir.
    if let Some(profile_file) = std::env::var_os("LLVM_PROFILE_FILE") {
        spec.environment
            .push(("LLVM_PROFILE_FILE".into(), profile_file));
    }
    spec
}

#[expect(
    clippy::too_many_lines,
    reason = "This binary exercises the complete process lifecycle contract in one isolated fixture run."
)]
async fn check() {
    let token = CancellationToken::new();
    let mut echo = spec("echo");
    echo.stdin = Some(vec![0xff; 1024 * 1024]);
    echo.stdout_limit = 1024 * 1024;
    let output = run(echo, &token, |_, _| {}).await;
    assert!(output.status.unwrap().success());
    assert!(
        output.cleanup_error.is_none(),
        "cleanup failed: {:?}",
        output.cleanup_error
    );
    assert_eq!(output.stdout.bytes, vec![0xff; 1024 * 1024]);
    assert!(!output.stdout.truncated);

    let mut streams = spec("streams");
    streams.stdin = Some(vec![b'i'; 1024 * 1024]);
    streams.stdout_limit = 8;
    streams.stderr_limit = 9;
    let observed = Arc::new(Mutex::new((Vec::new(), Vec::new())));
    let captured = observed.clone();
    let output = run(streams, &token, move |stream, bytes| {
        let mut observed = captured.lock().unwrap();
        match stream {
            ProcessStream::Stdout => observed.0.extend_from_slice(bytes),
            ProcessStream::Stderr => observed.1.extend_from_slice(bytes),
        }
    })
    .await;
    assert!(output.status.unwrap().success());
    assert_eq!(output.stdout.bytes.len(), 8);
    assert_eq!(output.stderr.bytes.len(), 9);
    assert!(output.stdout.truncated && output.stderr.truncated);
    {
        let observed = observed.lock().unwrap();
        assert_eq!(observed.0, vec![b'o'; 8]);
        assert_eq!(observed.1, vec![b'e'; 9]);
    }

    let mut exact = spec("streams");
    exact.stdout_limit = 128 * 1024;
    exact.stderr_limit = 128 * 1024;
    let output = run(exact, &token, |_, _| {}).await;
    assert!(output.status.unwrap().success());
    assert_eq!(output.stdout.bytes, vec![b'o'; 128 * 1024]);
    assert_eq!(output.stderr.bytes, vec![b'e'; 128 * 1024]);
    assert!(!output.stdout.truncated && !output.stderr.truncated);

    let mut no_output = spec("streams");
    no_output.stdout_limit = 0;
    no_output.stderr_limit = 0;
    let callbacks = AtomicUsize::new(0);
    let output = run(no_output, &token, |_, bytes| {
        callbacks.fetch_add(bytes.len(), Ordering::SeqCst);
    })
    .await;
    assert!(output.status.unwrap().success());
    assert!(output.stdout.bytes.is_empty() && output.stderr.bytes.is_empty());
    assert!(output.stdout.truncated && output.stderr.truncated);
    assert_eq!(callbacks.load(Ordering::SeqCst), 0);

    let injected = spec("env").env("PROCESS_FIXTURE_VALUE", "provided");
    let output = run(injected, &token, |_, _| {}).await;
    assert!(output.status.unwrap().success());
    assert_eq!(output.stdout.bytes, b"provided");

    let mut inherited = ProcessSpec::new(std::env::current_exe().unwrap(), Duration::from_secs(5));
    inherited.args = vec!["--fixture".into(), "inherited-env".into()];
    let output = run(inherited, &token, |_, _| {}).await;
    assert!(output.status.unwrap().success());
    assert_eq!(output.stdout.bytes, b"1");

    let output = run(spec("echo"), &token, |_, _| {}).await;
    assert!(output.status.unwrap().success());
    assert!(output.stdout.bytes.is_empty());

    check_stdin_boundaries().await;

    let mut sensitive = spec("echo");
    sensitive.stdin = Some(b"secret-sentinel".to_vec());
    sensitive.args.push("argument-sentinel".into());
    sensitive.environment = vec![("KEY".into(), "secret-sentinel".into())];
    let debug = format!("{sensitive:?}");
    assert!(!debug.contains("secret-sentinel"));
    assert!(!debug.contains("argument-sentinel"));

    let cancelled = CancellationToken::new();
    cancelled.cancel();
    let output = run(spec("echo"), &cancelled, |_, _| {}).await;
    assert!(matches!(
        output.status,
        Err(ProcessFailure::CancelledBeforeStart)
    ));
    assert!(!output.started);

    let mut zero = spec("echo");
    zero.timeout = Duration::ZERO;
    let output = run(zero, &CancellationToken::new(), |_, _| {}).await;
    assert!(matches!(output.status, Err(ProcessFailure::TimedOut)));
    assert!(!output.started);

    let mut missing = spec("echo");
    missing.program =
        std::env::temp_dir().join(format!("aster-forge-missing-{}", std::process::id()));
    let output = run(missing, &CancellationToken::new(), |_, _| {}).await;
    assert!(matches!(output.status, Err(ProcessFailure::Spawn(_))));
    assert!(!output.started);

    check_tree_shutdown(false).await;
    check_tree_shutdown(true).await;
    check_orphan_exit().await;
    check_dropped_future().await;
}

async fn check_stdin_boundaries() {
    let mut closed = spec("closed");
    closed.stdin = Some(vec![b'i'; 8 * 1024 * 1024]);
    let output = run(closed, &CancellationToken::new(), |_, _| {}).await;
    assert!(output.status.unwrap().success());
    assert!(output.stdin_error.is_some());

    #[cfg(unix)]
    check_stdin_error_survives_timeout().await;
}

#[cfg(unix)]
async fn check_stdin_error_survives_timeout() {
    let mut spec = spec("close-stdin-sleep");
    spec.stdin = Some(vec![b'i'; 8 * 1024 * 1024]);
    spec.timeout = Duration::from_secs(2);
    let output = run(spec, &CancellationToken::new(), |_, _| {}).await;
    assert!(matches!(output.status, Err(ProcessFailure::TimedOut)));
    assert!(output.started);
    assert_eq!(output.stdin_error, Some(std::io::ErrorKind::BrokenPipe));
}

async fn check_orphan_exit() {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: subreaper state is process-local to this isolated test binary.
        let result = unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) };
        assert_eq!(result, 0, "enable subreaper mode for orphan cleanup test");
    }
    let (ready, received) = tokio::sync::oneshot::channel();
    let state = Mutex::new((Vec::new(), Some(ready)));
    let output = run(
        spec("orphan"),
        &CancellationToken::new(),
        |stream, bytes| {
            if stream == ProcessStream::Stdout {
                let mut state = state.lock().unwrap();
                record_pid(&mut state, bytes);
            }
        },
    )
    .await;
    assert!(output.status.unwrap().success());
    let pid = received.await.unwrap();
    assert!(
        !process_alive(pid),
        "descendant process survived run completion"
    );
    wait_until_dead(pid).await;
}

async fn check_dropped_future() {
    let (ready, received) = tokio::sync::oneshot::channel();
    let token = CancellationToken::new();
    let task_token = token.clone();
    let state = Arc::new(Mutex::new((Vec::new(), Some(ready))));
    let callback_state = state.clone();
    let running = tokio::spawn(async move {
        run(spec("tree"), &task_token, move |stream, bytes| {
            if stream == ProcessStream::Stdout {
                let mut state = callback_state.lock().unwrap();
                record_pid(&mut state, bytes);
            }
        })
        .await
    });
    let pid = tokio::time::timeout(Duration::from_secs(10), received)
        .await
        .unwrap()
        .unwrap();
    assert!(process_alive(pid));
    running.abort();
    let _ = running.await;
    wait_until_dead(pid).await;
}

async fn check_tree_shutdown(timeout: bool) {
    let token = CancellationToken::new();
    let (ready, received) = tokio::sync::oneshot::channel();
    let state = Mutex::new((Vec::new(), Some(ready)));
    let mut tree = spec("tree");
    tree.timeout = if timeout {
        Duration::from_secs(3)
    } else {
        Duration::from_secs(30)
    };
    let running = run(tree, &token, |stream, bytes| {
        if stream != ProcessStream::Stdout {
            return;
        }
        let mut state = state.lock().unwrap();
        record_pid(&mut state, bytes);
    });
    tokio::pin!(running);
    let pid = tokio::select! {
        output = &mut running => panic!("tree exited before ready: status={:?}, stderr={:?}", output.status, String::from_utf8_lossy(&output.stderr.bytes)),
        ready = tokio::time::timeout(Duration::from_secs(10), received) => ready.expect("tree fixture did not become ready").expect("tree readiness sender was dropped"),
    };
    assert!(process_alive(pid));
    if !timeout {
        token.cancel();
    }
    let output = running.await;
    assert!(
        matches!(output.status, Err(ProcessFailure::Cancelled) if !timeout)
            || matches!(output.status, Err(ProcessFailure::TimedOut) if timeout)
    );
    assert!(output.started);
    assert!(
        !process_alive(pid),
        "descendant process survived run completion"
    );
    wait_until_dead(pid).await;
}

fn record_pid(state: &mut (Vec<u8>, Option<tokio::sync::oneshot::Sender<u32>>), bytes: &[u8]) {
    state.0.extend_from_slice(bytes);
    let Some(end) = state.0.iter().position(|byte| *byte == b'\n') else {
        return;
    };
    let Ok(pid) = std::str::from_utf8(&state.0[..end])
        .unwrap_or_default()
        .trim()
        .parse::<u32>()
    else {
        return;
    };
    if let Some(sender) = state.1.take() {
        let _ = sender.send(pid);
    }
}

async fn wait_until_dead(pid: u32) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while process_alive(pid) && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        !process_alive(pid),
        "descendant process {pid} survived cleanup"
    );
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(windows)]
fn process_alive(pid: u32) -> bool {
    std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .output()
        .is_ok_and(|output| {
            String::from_utf8_lossy(&output.stdout).contains(&format!(",\"{pid}\","))
        })
}
