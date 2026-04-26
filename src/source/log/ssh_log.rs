use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::errors::LensError;
use crate::session::TruncatedAt;
use crate::source::ssh_tunnel::{validate_ssh_host, validate_ssh_path};

pub const HARD_CAP_LINES: usize = 10_000;
pub const BOUNDED_TAIL_FOR_GREP: usize = 10_000;
const STDERR_CAP_BYTES: usize = 8 * 1024;

#[derive(Debug, Clone, Copy)]
pub struct SshLogCaps {
    pub line_bytes: usize,
    pub bytes: usize,
    pub timeout: Duration,
}

#[derive(Debug)]
pub struct SshLogSource {
    profile_name: String,
    host: String,
    path: String,
    max_line_bytes: usize,
    max_total_bytes: usize,
    timeout: Duration,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SshLogOutput {
    pub lines: Vec<String>,
    pub truncated_at: Vec<TruncatedAt>,
}

impl SshLogSource {
    pub fn new(
        profile_name: impl Into<String>,
        host: impl Into<String>,
        path: impl Into<String>,
        caps: SshLogCaps,
    ) -> Result<Self, LensError> {
        let profile_name = profile_name.into();
        let host = host.into();
        let path = path.into();
        validate_ssh_host(&host)
            .map_err(|err| source_error(&profile_name, err.to_string(), None))?;
        validate_ssh_path(&path)
            .map_err(|err| source_error(&profile_name, err.to_string(), None))?;
        Ok(Self {
            profile_name,
            host,
            path,
            max_line_bytes: caps.line_bytes,
            max_total_bytes: caps.bytes,
            timeout: caps.timeout,
        })
    }

    pub fn profile_name(&self) -> &str {
        &self.profile_name
    }

    pub fn tail_argv(&self, lines: usize) -> Vec<String> {
        tail_argv(&self.host, &self.path, lines)
    }

    pub async fn tail(&self, lines: usize) -> Result<SshLogOutput, LensError> {
        let argv = self.tail_argv(lines);
        let mut cmd = Command::new(&argv[0]);
        cmd.args(&argv[1..]);
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|err| source_error(&self.profile_name, err.to_string(), None))?;
        let stdout = child.stdout.take().ok_or_else(|| LensError::Internal {
            detail: "ssh stdout was not piped".to_string(),
        })?;
        let stderr = child.stderr.take().ok_or_else(|| LensError::Internal {
            detail: "ssh stderr was not piped".to_string(),
        })?;

        let read_result = tokio::time::timeout(self.timeout, async {
            let stdout_task = read_capped(stdout, self.max_total_bytes.saturating_add(1));
            let stderr_task = read_capped(stderr, STDERR_CAP_BYTES);
            let wait_task = child.wait();
            let (stdout, stderr, status) = tokio::join!(stdout_task, stderr_task, wait_task);
            Ok::<_, std::io::Error>((stdout?, stderr?, status?))
        })
        .await;

        let (status, mut stdout, stderr) = match read_result {
            Ok(Ok((stdout, stderr, status))) => (status, stdout, stderr),
            Ok(Err(err)) => {
                return Err(source_error(&self.profile_name, err.to_string(), None));
            }
            Err(_) => {
                let _ = child.kill().await;
                return Err(LensError::Truncated(TruncatedAt::Timeout));
            }
        };

        if !status.success() {
            return Err(source_error(
                &self.profile_name,
                format!("ssh returned {:?}", status.code()),
                Some(String::from_utf8_lossy(&stderr).into_owned()),
            ));
        }

        let mut truncated_at = Vec::new();
        if stdout.len() > self.max_total_bytes {
            truncated_at.push(TruncatedAt::Bytes);
            stdout.truncate(self.max_total_bytes);
        }
        let (lines, line_truncated) =
            split_and_cap_lines_with_truncation(&stdout, self.max_line_bytes);
        if line_truncated {
            truncated_at.push(TruncatedAt::LineBytes);
        }
        Ok(SshLogOutput {
            lines,
            truncated_at,
        })
    }

    pub async fn grep(
        &self,
        pattern: &str,
        level: Option<&str>,
        limit: usize,
    ) -> Result<SshLogOutput, LensError> {
        let re = regex::Regex::new(pattern).map_err(|_| LensError::SourceError {
            source_name: self.profile_name.clone(),
            detail: "invalid log grep regex".to_string(),
            sql: None,
            stderr: None,
        })?;
        let level_re = level
            .map(|level| regex::Regex::new(&format!(r"(?i)\b{}\b", regex::escape(level))))
            .transpose()
            .map_err(|_| LensError::SourceError {
                source_name: self.profile_name.clone(),
                detail: "invalid log level filter".to_string(),
                sql: None,
                stderr: None,
            })?;
        let output = self.tail(BOUNDED_TAIL_FOR_GREP).await?;
        let lines = output
            .lines
            .into_iter()
            .filter(|line| {
                re.is_match(line) && level_re.as_ref().is_none_or(|level| level.is_match(line))
            })
            .take(limit)
            .collect();
        Ok(SshLogOutput {
            lines,
            truncated_at: output.truncated_at,
        })
    }
}

pub fn tail_argv(host: &str, path: &str, lines: usize) -> Vec<String> {
    vec![
        "ssh".to_string(),
        "--".to_string(),
        host.to_string(),
        "tail".to_string(),
        "-n".to_string(),
        lines.min(HARD_CAP_LINES).to_string(),
        "--".to_string(),
        path.to_string(),
    ]
}

pub fn split_and_cap_lines(raw: &[u8], max_line_bytes: usize) -> Vec<String> {
    split_and_cap_lines_with_truncation(raw, max_line_bytes).0
}

pub fn split_and_cap_lines_with_truncation(
    raw: &[u8],
    max_line_bytes: usize,
) -> (Vec<String>, bool) {
    let mut truncated = false;
    let lines = raw
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| {
            truncated |= line.len() > max_line_bytes;
            let end = line.len().min(max_line_bytes);
            String::from_utf8_lossy(&line[..end]).into_owned()
        })
        .collect::<Vec<_>>();
    (lines, truncated)
}

async fn read_capped<R>(reader: R, max_bytes: usize) -> Result<Vec<u8>, std::io::Error>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut buf = Vec::with_capacity(max_bytes.min(64 * 1024));
    reader
        .take(max_bytes.min(u64::MAX as usize) as u64)
        .read_to_end(&mut buf)
        .await?;
    Ok(buf)
}

fn source_error(profile_name: &str, detail: String, stderr: Option<String>) -> LensError {
    LensError::SourceError {
        source_name: profile_name.to_string(),
        detail,
        sql: None,
        stderr,
    }
}
