use std::io::{self, Read};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const POLL_INTERVAL: Duration = Duration::from_millis(10);
const SERVE_OUTPUT_TIMEOUT: Duration = Duration::from_secs(30);

pub fn serve_output(command: &mut Command) -> Output {
    command_output_with_timeout(command, SERVE_OUTPUT_TIMEOUT, "gaze-lens serve")
}

pub fn command_output_with_timeout(
    command: &mut Command,
    timeout: Duration,
    context: &str,
) -> Output {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let child = command
        .spawn()
        .unwrap_or_else(|err| panic!("failed to spawn {context}: {err}"));
    wait_with_timeout(child, timeout, context)
}

fn wait_with_timeout(mut child: Child, timeout: Duration, context: &str) -> Output {
    let deadline = Instant::now() + timeout;

    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return collect_output(child, status)
                    .unwrap_or_else(|err| panic!("failed to read output from {context}: {err}"));
            }
            Ok(None) => {}
            Err(err) => panic_after_kill_and_reap(
                child,
                context,
                &format!("failed while waiting for child: {err}"),
            ),
        }

        let now = Instant::now();
        if now >= deadline {
            panic_after_kill_and_reap(child, context, &format!("timed out after {timeout:?}"));
        }

        thread::sleep(POLL_INTERVAL.min(deadline.saturating_duration_since(now)));
    }
}

fn collect_output(mut child: Child, status: ExitStatus) -> io::Result<Output> {
    let mut stdout = Vec::new();
    if let Some(mut pipe) = child.stdout.take() {
        pipe.read_to_end(&mut stdout)?;
    }

    let mut stderr = Vec::new();
    if let Some(mut pipe) = child.stderr.take() {
        pipe.read_to_end(&mut stderr)?;
    }

    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

fn panic_after_kill_and_reap(mut child: Child, context: &str, reason: &str) -> ! {
    let kill_result = child.kill();
    let status = child.wait().unwrap_or_else(|err| {
        panic!("{context} {reason}; kill result: {kill_result:?}; wait failed: {err}")
    });
    let output = collect_output(child, status).unwrap_or_else(|err| {
        panic!("{context} {reason}; kill result: {kill_result:?}; reaped with {status}; output read failed: {err}")
    });

    panic!(
        "{context} {reason}; kill result: {kill_result:?}; reaped with {status}; stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
