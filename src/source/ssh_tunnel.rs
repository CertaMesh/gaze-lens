use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TunnelSpec {
    pub ssh_host: String,
    pub local_port: u16,
    pub remote_host: String,
    pub remote_port: u16,
}

#[derive(Debug, thiserror::Error)]
pub enum SshError {
    #[error("invalid ssh host `{host}`: {reason}")]
    InvalidHost { host: String, reason: &'static str },
    #[error("invalid ssh path `{path}`: {reason}")]
    InvalidPath { path: String, reason: &'static str },
    #[error("ssh spawn failed: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("ssh exited non-zero: {0}")]
    NonZero(String),
}

pub struct SshTunnel {
    control_path: PathBuf,
    ssh_host: String,
}

impl SshTunnel {
    pub fn open(spec: &TunnelSpec) -> Result<Self, SshError> {
        let host = validate_ssh_host(&spec.ssh_host)?;
        let control_path = Self::control_path(spec.local_port);
        let status = Command::new("ssh")
            .args(open_argv_for_control_path(spec, host, &control_path)?)
            .status()?;
        if !status.success() {
            return Err(SshError::NonZero(format!(
                "ssh returned {:?}",
                status.code()
            )));
        }
        Ok(Self {
            control_path,
            ssh_host: host.to_string(),
        })
    }

    pub fn control_path(local_port: u16) -> PathBuf {
        std::env::temp_dir().join(format!(
            "gaze-ssh-{}-{}.sock",
            local_port,
            std::process::id()
        ))
    }

    pub fn close(&mut self) -> Result<(), SshError> {
        let host = validate_ssh_host(&self.ssh_host)?;
        let _ = Command::new("ssh")
            .args(close_argv_for_control_path(host, &self.control_path)?)
            .status();
        let _ = std::fs::remove_file(&self.control_path);
        Ok(())
    }
}

impl Drop for SshTunnel {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

pub fn validate_ssh_host(host: &str) -> Result<&str, SshError> {
    if host.is_empty() {
        return Err(invalid_host(host, "empty host"));
    }
    if host.len() > 253 {
        return Err(invalid_host(host, "host exceeds DNS length limit"));
    }
    if host.starts_with('-') {
        return Err(invalid_host(host, "host cannot start with '-'"));
    }
    if host
        .bytes()
        .any(|byte| !matches!(byte, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-'))
    {
        return Err(invalid_host(
            host,
            "host may only contain ASCII letters, digits, '.', '_', '-'",
        ));
    }
    Ok(host)
}

pub fn validate_ssh_path(path: &str) -> Result<&str, SshError> {
    // v1 deliberately accepts only exact ASCII paths. Operators needing
    // Unicode, globbing, or remote expansion can get those in a later
    // threat-modeled extension without weakening the default command builder.
    if path.is_empty() {
        return Err(invalid_path(path, "empty path"));
    }
    if path.len() > 4096 {
        return Err(invalid_path(path, "path exceeds PATH_MAX"));
    }
    if path.bytes().any(|byte| {
        matches!(
            byte,
            b';' | b'|' | b'&' | b'`' | b'$' | b'\n' | b'\r' | b'\0'
        )
    }) {
        return Err(invalid_path(path, "path contains shell metacharacters"));
    }
    if path
        .bytes()
        .any(|byte| matches!(byte, b'*' | b'?' | b'[' | b']'))
    {
        return Err(invalid_path(path, "path contains glob characters"));
    }
    if path.split('/').any(|segment| segment == "..") {
        return Err(invalid_path(path, "path contains traversal segment"));
    }
    if path.bytes().any(|byte| {
        !matches!(
            byte,
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'.' | b'_' | b'-'
        )
    }) {
        return Err(invalid_path(
            path,
            "path may only contain ASCII letters, digits, '/', '.', '_', '-'",
        ));
    }
    Ok(path)
}

pub fn open_argv(spec: &TunnelSpec) -> Result<Vec<String>, SshError> {
    let host = validate_ssh_host(&spec.ssh_host)?;
    open_argv_for_control_path(spec, host, &SshTunnel::control_path(spec.local_port))
}

pub fn close_argv(host: &str, control_path: &Path) -> Result<Vec<String>, SshError> {
    let host = validate_ssh_host(host)?;
    close_argv_for_control_path(host, control_path)
}

fn open_argv_for_control_path(
    spec: &TunnelSpec,
    host: &str,
    control_path: &Path,
) -> Result<Vec<String>, SshError> {
    Ok(vec![
        "-f".to_string(),
        "-N".to_string(),
        "-M".to_string(),
        "-S".to_string(),
        control_path.to_string_lossy().into_owned(),
        "-o".to_string(),
        "ExitOnForwardFailure=yes".to_string(),
        "-L".to_string(),
        format!(
            "{}:{}:{}",
            spec.local_port, spec.remote_host, spec.remote_port
        ),
        "--".to_string(),
        host.to_string(),
    ])
}

fn close_argv_for_control_path(host: &str, control_path: &Path) -> Result<Vec<String>, SshError> {
    Ok(vec![
        "-S".to_string(),
        control_path.to_string_lossy().into_owned(),
        "-O".to_string(),
        "exit".to_string(),
        "--".to_string(),
        host.to_string(),
    ])
}

fn invalid_host(host: &str, reason: &'static str) -> SshError {
    SshError::InvalidHost {
        host: host.to_string(),
        reason,
    }
}

fn invalid_path(path: &str, reason: &'static str) -> SshError {
    SshError::InvalidPath {
        path: path.to_string(),
        reason,
    }
}
