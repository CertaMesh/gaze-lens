use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::process::Command;

use crate::errors::LensError;
use crate::source::log::ssh_log::read_capped;
use crate::source::ssh_tunnel::{remote_argv, validate_ssh_login_host, validate_ssh_path};

const ENV_STDOUT_CAP_BYTES: usize = 64 * 1024;
const STDERR_CAP_BYTES: usize = 512;
const SSH_HARD_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatOutput {
    pub bytes: Vec<u8>,
    pub truncated: bool,
}

pub trait SshExec: Send + Sync {
    fn cat_capped(
        &self,
        host: &str,
        path: &Path,
        allow_new_host: bool,
    ) -> Result<CatOutput, LensError>;

    fn host_key_fingerprint(
        &self,
        _host: &str,
        _allow_new_host: bool,
    ) -> Result<String, LensError> {
        Err(LensError::Profile {
            detail: "host-key fingerprint unavailable".into(),
        })
    }
}

#[derive(Debug, Default)]
pub struct RealSsh;

impl SshExec for RealSsh {
    fn cat_capped(
        &self,
        host: &str,
        path: &Path,
        allow_new_host: bool,
    ) -> Result<CatOutput, LensError> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()
            .map_err(|err| LensError::Internal {
                detail: format!("failed to build ssh runtime: {err}"),
            })?;
        rt.block_on(real_cat_capped(host, path, allow_new_host))
    }

    fn host_key_fingerprint(&self, host: &str, allow_new_host: bool) -> Result<String, LensError> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()
            .map_err(|err| LensError::Internal {
                detail: format!("failed to build ssh runtime: {err}"),
            })?;
        rt.block_on(real_host_key_fingerprint(host, allow_new_host))
    }
}

pub fn hardened_cat_env_argv(
    host: &str,
    path: &Path,
    allow_new_host: bool,
) -> Result<Vec<String>, LensError> {
    let path_str = path.to_string_lossy();
    validate_ssh_login_host(host).map_err(|err| LensError::Profile {
        detail: format!("invalid ssh host: {err}"),
    })?;
    validate_ssh_path(&path_str).map_err(|err| LensError::Profile {
        detail: format!("invalid discovery env path: {err}"),
    })?;
    let strict = if allow_new_host {
        "StrictHostKeyChecking=accept-new"
    } else {
        "StrictHostKeyChecking=yes"
    };
    Ok(vec![
        "ssh".to_string(),
        "-o".to_string(),
        "BatchMode=yes".to_string(),
        "-o".to_string(),
        "ConnectTimeout=10".to_string(),
        "-o".to_string(),
        strict.to_string(),
        "--".to_string(),
        host.to_string(),
        "cat".to_string(),
        "--".to_string(),
        path_str.into_owned(),
    ])
}

async fn real_cat_capped(
    host: &str,
    path: &Path,
    allow_new_host: bool,
) -> Result<CatOutput, LensError> {
    let argv = hardened_cat_env_argv(host, path, allow_new_host)?;
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..]);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd.spawn().map_err(|err| LensError::Profile {
        detail: format!("ssh spawn failed: {err}"),
    })?;
    let stdout = child.stdout.take().ok_or_else(|| LensError::Internal {
        detail: "ssh stdout was not piped".into(),
    })?;
    let stderr = child.stderr.take().ok_or_else(|| LensError::Internal {
        detail: "ssh stderr was not piped".into(),
    })?;

    let read_result = tokio::time::timeout(SSH_HARD_TIMEOUT, async {
        let stdout_task = read_capped(stdout, ENV_STDOUT_CAP_BYTES.saturating_add(1));
        let stderr_task = read_capped(stderr, STDERR_CAP_BYTES);
        let wait_task = child.wait();
        let (stdout, stderr, status) = tokio::join!(stdout_task, stderr_task, wait_task);
        Ok::<_, std::io::Error>((stdout?, stderr?, status?))
    })
    .await;

    let (mut stdout, stderr, status) = match read_result {
        Ok(Ok((stdout, stderr, status))) => (stdout, stderr, status),
        Ok(Err(err)) => {
            return Err(LensError::Profile {
                detail: format!("ssh failed: {err}"),
            });
        }
        Err(_) => {
            let _ = child.kill().await;
            return Err(LensError::Profile {
                detail: "ssh timed out while reading remote .env".into(),
            });
        }
    };

    if !status.success() {
        let path_str = path.to_string_lossy();
        let mut stderr = String::from_utf8_lossy(&stderr).into_owned();
        stderr = stderr.replace(path_str.as_ref(), "<redacted>");
        return Err(LensError::Profile {
            detail: format!("ssh returned {:?}: {stderr}", status.code()),
        });
    }

    let truncated = stdout.len() > ENV_STDOUT_CAP_BYTES;
    if truncated {
        stdout.truncate(ENV_STDOUT_CAP_BYTES);
    }
    Ok(CatOutput {
        bytes: stdout,
        truncated,
    })
}

async fn real_host_key_fingerprint(host: &str, allow_new_host: bool) -> Result<String, LensError> {
    let argv = keyscan_argv(host, allow_new_host)?;
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..]);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|err| LensError::Profile {
        detail: format!("ssh-keyscan spawn failed: {err}"),
    })?;
    let stdout = child.stdout.take().ok_or_else(|| LensError::Internal {
        detail: "ssh-keyscan stdout was not piped".into(),
    })?;
    let stderr = child.stderr.take().ok_or_else(|| LensError::Internal {
        detail: "ssh-keyscan stderr was not piped".into(),
    })?;
    let read_result = tokio::time::timeout(Duration::from_secs(10), async {
        let stdout_task = read_capped(stdout, 4096);
        let stderr_task = read_capped(stderr, STDERR_CAP_BYTES);
        let wait_task = child.wait();
        let (stdout, stderr, status) = tokio::join!(stdout_task, stderr_task, wait_task);
        Ok::<_, std::io::Error>((stdout?, stderr?, status?))
    })
    .await;
    let (stdout, _stderr, status) = match read_result {
        Ok(Ok((stdout, stderr, status))) => (stdout, stderr, status),
        Ok(Err(err)) => {
            return Err(LensError::Profile {
                detail: format!("ssh-keyscan failed: {err}"),
            });
        }
        Err(_) => {
            let _ = child.kill().await;
            return Err(LensError::Profile {
                detail: "ssh-keyscan timed out".into(),
            });
        }
    };
    if !status.success() {
        return Err(LensError::Profile {
            detail: format!("ssh-keyscan returned {:?}", status.code()),
        });
    }
    let line = String::from_utf8_lossy(&stdout)
        .lines()
        .find(|line| !line.trim().is_empty() && !line.starts_with('#'))
        .map(str::to_string)
        .ok_or_else(|| LensError::Profile {
            detail: "ssh-keyscan returned no host key".into(),
        })?;
    Ok(line)
}

#[doc(hidden)]
#[derive(Clone, Default)]
pub struct MockSsh {
    responses: Arc<Mutex<MockResponses>>,
    calls: Arc<Mutex<Vec<(String, PathBuf, bool)>>>,
    fingerprints: Arc<Mutex<HashMap<String, String>>>,
}

type MockResponses = HashMap<(String, PathBuf), Result<CatOutput, String>>;

impl MockSsh {
    pub fn with_response(
        self,
        host: impl Into<String>,
        path: impl Into<PathBuf>,
        response: Result<CatOutput, String>,
    ) -> Self {
        self.responses
            .lock()
            .expect("mock ssh responses lock")
            .insert((host.into(), path.into()), response);
        self
    }

    pub fn with_fingerprint(self, host: impl Into<String>, fingerprint: impl Into<String>) -> Self {
        self.fingerprints
            .lock()
            .expect("mock ssh fingerprints lock")
            .insert(host.into(), fingerprint.into());
        self
    }

    pub fn call_count(&self) -> usize {
        self.calls.lock().expect("mock ssh calls lock").len()
    }
}

impl SshExec for MockSsh {
    fn cat_capped(
        &self,
        host: &str,
        path: &Path,
        allow_new_host: bool,
    ) -> Result<CatOutput, LensError> {
        self.calls.lock().expect("mock ssh calls lock").push((
            host.to_string(),
            path.to_path_buf(),
            allow_new_host,
        ));
        let key = (host.to_string(), path.to_path_buf());
        let response = self
            .responses
            .lock()
            .expect("mock ssh responses lock")
            .get(&key)
            .cloned()
            .unwrap_or_else(|| Err("mock ssh response missing".into()));
        response.map_err(|detail| {
            let path_str = path.to_string_lossy();
            LensError::Profile {
                detail: detail.replace(path_str.as_ref(), "<redacted>"),
            }
        })
    }

    fn host_key_fingerprint(&self, host: &str, _allow_new_host: bool) -> Result<String, LensError> {
        self.fingerprints
            .lock()
            .expect("mock ssh fingerprints lock")
            .get(host)
            .cloned()
            .ok_or_else(|| LensError::Profile {
                detail: "host-key fingerprint unavailable".into(),
            })
    }
}

pub(crate) fn keyscan_argv(host: &str, _allow_new_host: bool) -> Result<Vec<String>, LensError> {
    let _ = remote_argv(host, &["true"], "/tmp/placeholder").map_err(|err| LensError::Profile {
        detail: err.to_string(),
    })?;
    let keyscan_host = host.rsplit_once('@').map_or(host, |(_, h)| h);
    Ok(vec![
        "ssh-keyscan".into(),
        "-t".into(),
        "ed25519".into(),
        "--".into(),
        keyscan_host.into(),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hardened_cat_env_argv_asserts_exact_default() {
        let argv =
            hardened_cat_env_argv("deploy@app01", Path::new("/var/www/app/.env"), false).unwrap();
        assert_eq!(
            argv,
            vec![
                "ssh",
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=10",
                "-o",
                "StrictHostKeyChecking=yes",
                "--",
                "deploy@app01",
                "cat",
                "--",
                "/var/www/app/.env"
            ]
        );
    }

    #[test]
    fn hardened_cat_env_argv_allows_new_host_only_when_requested() {
        let argv =
            hardened_cat_env_argv("deploy@app01", Path::new("/var/www/app/.env"), true).unwrap();
        assert!(
            argv.iter()
                .any(|arg| arg == "StrictHostKeyChecking=accept-new")
        );
    }

    #[test]
    fn mock_ssh_redacts_path_in_error() {
        let path = Path::new("/var/www/app/.env");
        let ssh = MockSsh::default().with_response(
            "deploy@app01",
            path,
            Err("cat: /var/www/app/.env: Permission denied".into()),
        );
        let err = ssh.cat_capped("deploy@app01", path, false).unwrap_err();
        assert!(!err.to_string().contains("/var/www/app/.env"));
        assert!(err.to_string().contains("<redacted>"));
    }
}
