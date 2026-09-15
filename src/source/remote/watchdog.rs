//! One process-wide admission owner, independent of Tokio and privacy locks.
use super::{Result, failure};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

static SUPERVISOR: OnceLock<Arc<Supervisor>> = OnceLock::new();
struct Supervisor {
    state: Mutex<State>,
    changed: Condvar,
}
enum State {
    Idle,
    Running(Instant),
    Expired,
}
pub(crate) struct Admission {
    supervisor: Arc<Supervisor>,
    deadline: Instant,
}

fn terminate() -> ! {
    // SAFETY: _exit terminates the whole Unix process without Rust unwinding,
    // atexit handlers, buffered diagnostics, or an abort-generated core dump.
    unsafe { libc::_exit(124) }
}
pub(crate) fn fixed_panic_hook() {
    std::panic::set_hook(Box::new(|_| terminate()));
}

pub(crate) fn enable() -> Result<()> {
    fixed_panic_hook();
    // Do not initialize payload-capable dependency logging in the raw modes.
    tracing::subscriber::set_global_default(tracing::subscriber::NoSubscriber::default())
        .map_err(|_| failure())?;
    let supervisor = Arc::new(Supervisor {
        state: Mutex::new(State::Idle),
        changed: Condvar::new(),
    });
    let worker = Arc::clone(&supervisor);
    std::thread::Builder::new()
        .name("privacy-deadline".into())
        .spawn(move || {
            let mut state = worker.state.lock().unwrap_or_else(|_| terminate());
            loop {
                match *state {
                    State::Idle => {
                        state = worker.changed.wait(state).unwrap_or_else(|_| terminate())
                    }
                    State::Expired => terminate(),
                    State::Running(deadline) => {
                        let now = Instant::now();
                        if now >= deadline {
                            *state = State::Expired;
                            terminate();
                        }
                        state = worker
                            .changed
                            .wait_timeout(state, deadline - now)
                            .unwrap_or_else(|_| terminate())
                            .0;
                    }
                }
            }
        })
        .map_err(|_| failure())?;
    SUPERVISOR.set(supervisor).map_err(|_| failure())
}

#[cfg(test)]
pub(crate) static TEST_TIMEOUT_MS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(90_000);

pub(crate) fn admit() -> Result<Option<Admission>> {
    #[cfg(test)]
    let timeout = Duration::from_millis(TEST_TIMEOUT_MS.load(std::sync::atomic::Ordering::Relaxed));
    #[cfg(not(test))]
    let timeout = Duration::from_secs(90);
    admit_for(timeout)
}
fn admit_for(timeout: Duration) -> Result<Option<Admission>> {
    let Some(supervisor) = SUPERVISOR.get() else {
        return Ok(None);
    };
    let mut state = supervisor.state.lock().map_err(|_| failure())?;
    if !matches!(*state, State::Idle) {
        return Err(failure());
    }
    let deadline = Instant::now() + timeout;
    *state = State::Running(deadline);
    supervisor.changed.notify_one();
    Ok(Some(Admission {
        supervisor: Arc::clone(supervisor),
        deadline,
    }))
}
impl Admission {
    pub(crate) fn complete(self) {
        let mut state = self.supervisor.state.lock().unwrap_or_else(|_| terminate());
        if Instant::now() >= self.deadline
            || !matches!(*state,State::Running(d) if d==self.deadline)
        {
            *state = State::Expired;
            terminate();
        }
        *state = State::Idle;
        self.supervisor.changed.notify_one();
    }
}
// No Drop implementation: cancellation/unwind cannot disarm active supervision.

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn watchdog_child() {
        let Ok(mode) = std::env::var("GAZE_WATCHDOG_UNIT_CHILD") else {
            return;
        };
        enable().unwrap();
        let guard = admit_for(Duration::from_millis(120)).unwrap().unwrap();
        match mode.as_str() {
            "complete" => {
                guard.complete();
                assert!(admit_for(Duration::from_millis(120)).unwrap().is_some());
            }
            "cancel" => drop(guard),
            "contention" => {
                assert!(admit().is_err());
                drop(guard);
            }
            "panic" => panic!("RAW_PANIC_CANARY secret@example.test"),
            "stderr" => {
                use std::io::Write;
                let mut stderr = std::io::stderr().lock();
                loop {
                    let _ = stderr.write_all(&[b'x'; 8192]);
                }
            }
            "timely" => {
                guard.complete();
                std::thread::sleep(Duration::from_millis(240));
                return;
            }
            _ => drop(guard),
        }
        // Synchronous blocking privacy work cannot stall the independent thread.
        std::thread::sleep(Duration::from_secs(30));
    }
    #[test]
    fn watchdog_kills_without_stderr_unwind_or_cancellation_disarm() {
        use std::process::{Command, Stdio};
        for mode in [
            "stall",
            "cancel",
            "contention",
            "panic",
            "stderr",
            "complete",
            "timely",
        ] {
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "source::remote::watchdog::tests::watchdog_child",
                    "--nocapture",
                ])
                .env("GAZE_WATCHDOG_UNIT_CHILD", mode)
                .env("RUST_LOG", "trace")
                .env("GAZE_LENS_VERBOSE_ERRORS", "1")
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let start = Instant::now();
            let status = loop {
                if let Some(status) = child.try_wait().unwrap() {
                    break status;
                }
                if start.elapsed() > Duration::from_secs(4) {
                    child.kill().unwrap();
                    panic!("watchdog did not terminate {mode}");
                }
                std::thread::sleep(Duration::from_millis(10));
            };
            assert_eq!(
                status.code(),
                Some(if mode == "timely" { 0 } else { 124 }),
                "{mode}"
            );
            use std::io::Read;
            let mut stderr = String::new();
            child
                .stderr
                .take()
                .unwrap()
                .read_to_string(&mut stderr)
                .unwrap();
            assert!(!stderr.contains("RAW_PANIC_CANARY"));
            assert!(!stderr.contains("secret@example.test"));
        }
    }
}
