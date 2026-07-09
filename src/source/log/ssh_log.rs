use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::process::Stdio;
use std::time::{Duration, Instant};

use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::errors::LensError;
use crate::session::TruncatedAt;
use crate::source::ssh_tunnel::{validate_ssh_login_host, validate_ssh_path};

pub const HARD_CAP_LINES: usize = 10_000;
pub const BOUNDED_TAIL_FOR_GREP: usize = 10_000;
const SSH_CONNECT_TIMEOUT_SECS: u64 = 10;
const STDERR_CAP_BYTES: usize = 8 * 1024;
// Short-lived only: amortizes repeated log_grep SSH tails during live triage
// without creating a raw log store beyond the running process.
const GREP_WINDOW_CACHE_TTL: Duration = Duration::from_secs(3);

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
    grep_window_cache: WindowCache,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SshLogOutput {
    pub lines: Vec<String>,
    pub truncated_at: Vec<TruncatedAt>,
    pub bytes: usize,
    pub metadata: Option<SshLogMetadata>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SshLogMetadata {
    pub operation: &'static str,
    pub status: &'static str,
    pub profile: String,
    pub source_kind: &'static str,
    pub host: String,
    pub path: String,
    pub pattern: String,
    pub level: Option<String>,
    pub requested_limit: usize,
    pub tail_window_lines: usize,
    pub searched_lines: usize,
    pub matched_lines: usize,
    pub returned_lines: usize,
    pub searched_bytes: usize,
    pub truncated_at: Vec<TruncatedAt>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct WindowCacheKey {
    host: String,
    path: String,
    window_lines: usize,
}

impl WindowCacheKey {
    fn new(
        host: impl Into<String>,
        path: impl Into<String>,
        window_lines: usize,
    ) -> WindowCacheKey {
        WindowCacheKey {
            host: host.into(),
            path: path.into(),
            window_lines,
        }
    }
}

#[derive(Debug)]
struct CachedWindow {
    minted_at: Instant,
    output: SshLogOutput,
}

struct WindowCache {
    ttl: Duration,
    now: Box<dyn Fn() -> Instant + Send + Sync>,
    entries: std::sync::Mutex<HashMap<WindowCacheKey, CachedWindow>>,
}

impl fmt::Debug for WindowCache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WindowCache")
            .field("ttl", &self.ttl)
            .finish_non_exhaustive()
    }
}

impl WindowCache {
    fn new(ttl: Duration) -> WindowCache {
        Self::with_clock(ttl, Instant::now)
    }

    fn with_clock(ttl: Duration, now: impl Fn() -> Instant + Send + Sync + 'static) -> WindowCache {
        WindowCache {
            ttl,
            now: Box::new(now),
            entries: std::sync::Mutex::new(HashMap::new()),
        }
    }

    async fn get_or_fetch<F, Fut>(
        &self,
        key: WindowCacheKey,
        fetch: F,
    ) -> Result<SshLogOutput, LensError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<SshLogOutput, LensError>>,
    {
        let now = (self.now)();
        if let Some(output) = self.fresh(&key, now) {
            return Ok(output);
        }

        let output = fetch().await?;
        self.store(key, output.clone(), (self.now)());
        Ok(output)
    }

    async fn get_or_fetch_refresh<F, Fut>(
        &self,
        key: WindowCacheKey,
        refresh: bool,
        fetch: F,
    ) -> Result<SshLogOutput, LensError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<SshLogOutput, LensError>>,
    {
        if !refresh {
            return self.get_or_fetch(key, fetch).await;
        }

        let output = fetch().await?;
        self.store(key, output.clone(), (self.now)());
        Ok(output)
    }

    fn fresh(&self, key: &WindowCacheKey, now: Instant) -> Option<SshLogOutput> {
        self.entries
            .lock()
            .expect("window cache")
            .get(key)
            .filter(|entry| now.saturating_duration_since(entry.minted_at) <= self.ttl)
            .map(|entry| entry.output.clone())
    }

    fn store(&self, key: WindowCacheKey, output: SshLogOutput, minted_at: Instant) {
        self.entries
            .lock()
            .expect("window cache")
            .insert(key, CachedWindow { minted_at, output });
    }
}

impl SshLogOutput {
    pub fn into_text(self) -> String {
        let lines = self.lines.join("\n");
        let Some(metadata) = self.metadata else {
            return lines;
        };
        let metadata = serde_json::to_string(&metadata).expect("ssh log metadata should serialize");
        if lines.is_empty() {
            metadata
        } else {
            format!("{metadata}\n{lines}")
        }
    }
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
        // #504: accept `user@host` so the log host can carry an explicit login
        // user, consistent with the runtime argv builder (`tail_argv` →
        // `validate_ssh_login_host`) and `--discover-ssh-host`.
        validate_ssh_login_host(&host)
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
            grep_window_cache: WindowCache::new(GREP_WINDOW_CACHE_TTL),
        })
    }

    pub fn profile_name(&self) -> &str {
        &self.profile_name
    }

    pub fn tail_argv(&self, lines: usize) -> Vec<String> {
        tail_argv(&self.host, &self.path, lines)
    }

    pub async fn tail(&self, lines: usize) -> Result<SshLogOutput, LensError> {
        self.tail_for_operation(lines, "log_tail").await
    }

    async fn tail_for_operation(
        &self,
        lines: usize,
        operation: &str,
    ) -> Result<SshLogOutput, LensError> {
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
                return Err(timeout_error(
                    &self.profile_name,
                    "ssh log command/read",
                    operation,
                    &self.host,
                    &self.path,
                    self.timeout,
                ));
            }
        };

        if !status.success() {
            if ssh_stderr_indicates_connect_timeout(&stderr) {
                return Err(timeout_error(
                    &self.profile_name,
                    "ssh connect",
                    operation,
                    &self.host,
                    &self.path,
                    Duration::from_secs(SSH_CONNECT_TIMEOUT_SECS),
                ));
            }
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
        let bytes = stdout.len();
        Ok(SshLogOutput {
            lines,
            truncated_at,
            bytes,
            metadata: None,
        })
    }

    pub async fn grep(
        &self,
        pattern: &str,
        level: Option<&str>,
        limit: usize,
    ) -> Result<SshLogOutput, LensError> {
        self.grep_with_tail_fetcher(pattern, level, limit, || {
            self.tail_for_operation(BOUNDED_TAIL_FOR_GREP, "log_grep")
        })
        .await
    }

    pub async fn grep_window(
        &self,
        pattern: &str,
        level: Option<&str>,
        limit: usize,
        refresh: bool,
    ) -> Result<SshLogOutput, LensError> {
        self.grep_window_with_tail_fetcher(pattern, level, limit, refresh, || {
            self.tail_for_operation(BOUNDED_TAIL_FOR_GREP, "log_grep")
        })
        .await
    }

    async fn grep_window_with_tail_fetcher<F, Fut>(
        &self,
        pattern: &str,
        level: Option<&str>,
        limit: usize,
        refresh: bool,
        fetch: F,
    ) -> Result<SshLogOutput, LensError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<SshLogOutput, LensError>>,
    {
        let level = non_empty_level(level);
        let output = self
            .grep_window_cache
            .get_or_fetch_refresh(
                self.grep_window_cache_key(BOUNDED_TAIL_FOR_GREP),
                refresh,
                fetch,
            )
            .await?;
        Ok(self.full_grep_window_output(pattern, level, limit, output))
    }

    async fn grep_with_tail_fetcher<F, Fut>(
        &self,
        pattern: &str,
        level: Option<&str>,
        limit: usize,
        fetch: F,
    ) -> Result<SshLogOutput, LensError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<SshLogOutput, LensError>>,
    {
        let level = non_empty_level(level);
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
        let output = self
            .grep_window_cache
            .get_or_fetch(self.grep_window_cache_key(BOUNDED_TAIL_FOR_GREP), fetch)
            .await?;
        Ok(self.filter_grep_output(pattern, level, limit, output, &re, level_re.as_ref()))
    }

    fn grep_window_cache_key(&self, window_lines: usize) -> WindowCacheKey {
        WindowCacheKey::new(self.host.as_str(), self.path.as_str(), window_lines)
    }

    fn filter_grep_output(
        &self,
        pattern: &str,
        level: Option<&str>,
        limit: usize,
        output: SshLogOutput,
        re: &regex::Regex,
        level_re: Option<&regex::Regex>,
    ) -> SshLogOutput {
        let level = non_empty_level(level);
        let searched_lines = output.lines.len();
        let searched_bytes = output.bytes;
        let mut truncated_at = output.truncated_at;
        let mut matched_lines = 0usize;
        let mut lines = Vec::new();
        for line in output.lines {
            if re.is_match(&line) && level_re.is_none_or(|level| level.is_match(&line)) {
                matched_lines += 1;
                if lines.len() < limit {
                    lines.push(line);
                }
            }
        }
        if matched_lines > lines.len() {
            push_truncation(&mut truncated_at, TruncatedAt::Rows);
        }
        let metadata = grep_metadata_required(matched_lines, &truncated_at).then(|| {
            let status = grep_status(matched_lines, &truncated_at);
            SshLogMetadata {
                operation: "log_grep",
                status,
                profile: self.profile_name.clone(),
                source_kind: "ssh_log",
                host: self.host.clone(),
                path: self.path.clone(),
                pattern: pattern.to_string(),
                level: level.map(ToOwned::to_owned),
                requested_limit: limit,
                tail_window_lines: BOUNDED_TAIL_FOR_GREP,
                searched_lines,
                matched_lines,
                returned_lines: lines.len(),
                searched_bytes,
                truncated_at: truncated_at.clone(),
            }
        });
        SshLogOutput {
            lines,
            truncated_at,
            bytes: searched_bytes,
            metadata,
        }
    }

    fn full_grep_window_output(
        &self,
        pattern: &str,
        level: Option<&str>,
        limit: usize,
        output: SshLogOutput,
    ) -> SshLogOutput {
        let level = non_empty_level(level);
        let searched_lines = output.lines.len();
        let searched_bytes = output.bytes;
        let truncated_at = output.truncated_at;
        let metadata = Some(SshLogMetadata {
            operation: "log_grep",
            status: grep_status(searched_lines, &truncated_at),
            profile: self.profile_name.clone(),
            source_kind: "ssh_log",
            host: self.host.clone(),
            path: self.path.clone(),
            pattern: pattern.to_string(),
            level: level.map(ToOwned::to_owned),
            requested_limit: limit,
            tail_window_lines: BOUNDED_TAIL_FOR_GREP,
            searched_lines,
            matched_lines: searched_lines,
            returned_lines: searched_lines,
            searched_bytes,
            truncated_at: truncated_at.clone(),
        });
        SshLogOutput {
            lines: output.lines,
            truncated_at,
            bytes: searched_bytes,
            metadata,
        }
    }
}

pub fn tail_argv(host: &str, path: &str, lines: usize) -> Vec<String> {
    let host = validate_ssh_login_host(host).expect("SshLogSource validates host before tail_argv");
    let path = validate_ssh_path(path).expect("SshLogSource validates path before tail_argv");
    let capped_lines = lines.min(HARD_CAP_LINES).to_string();
    vec![
        "ssh".to_string(),
        "-o".to_string(),
        "BatchMode=yes".to_string(),
        "-o".to_string(),
        format!("ConnectTimeout={SSH_CONNECT_TIMEOUT_SECS}"),
        "--".to_string(),
        host.to_string(),
        "tail".to_string(),
        "-n".to_string(),
        capped_lines,
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

/// Used by `tail` and `cat` shell-outs; cap defends against unbounded remote
/// stdout under `tokio::time::timeout`.
pub(crate) async fn read_capped<R>(reader: R, max_bytes: usize) -> Result<Vec<u8>, std::io::Error>
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

fn timeout_error(
    profile_name: &str,
    phase: &str,
    operation: &str,
    host: &str,
    path: &str,
    timeout: Duration,
) -> LensError {
    LensError::OperationTimeout {
        phase: phase.to_string(),
        operation: operation.to_string(),
        timeout_secs: timeout.as_secs(),
        context: Some(format!("profile={profile_name} host={host} path={path}")),
    }
}

fn ssh_stderr_indicates_connect_timeout(stderr: &[u8]) -> bool {
    let stderr = String::from_utf8_lossy(stderr).to_ascii_lowercase();
    stderr.contains("connection timed out") || stderr.contains("operation timed out")
}

fn non_empty_level(level: Option<&str>) -> Option<&str> {
    level.filter(|value| !value.is_empty())
}

fn grep_metadata_required(_: usize, _: &[TruncatedAt]) -> bool {
    true
}

fn grep_status(matched_lines: usize, truncated_at: &[TruncatedAt]) -> &'static str {
    match (matched_lines, truncated_at.is_empty()) {
        (0, true) => "no_matches",
        (0, false) => "no_matches_truncated",
        (_, false) => "truncated",
        (_, true) => "matches",
    }
}

fn push_truncation(truncated_at: &mut Vec<TruncatedAt>, reason: TruncatedAt) {
    if !truncated_at.contains(&reason) {
        truncated_at.push(reason);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::errors::sanitize_error;
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };
    use std::time::Instant;

    #[tokio::test]
    async fn window_cache_reuses_fresh_window_for_same_key() {
        let clock = test_clock();
        let cache = WindowCache::with_clock(Duration::from_secs(5), clock.now_fn());
        let key = WindowCacheKey::new("app.example", "/var/log/app.log", 10);
        let fetches = Arc::new(AtomicUsize::new(0));

        let first = cache
            .get_or_fetch(key.clone(), {
                let fetches = Arc::clone(&fetches);
                || async move {
                    fetches.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, LensError>(sample_window("ERROR release_id=43301 first", 33))
                }
            })
            .await
            .expect("first fetch");
        clock.advance(Duration::from_secs(1));
        let second = cache
            .get_or_fetch(key, {
                let fetches = Arc::clone(&fetches);
                || async move {
                    fetches.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, LensError>(sample_window("ERROR release_id=43301 second", 34))
                }
            })
            .await
            .expect("cached fetch");

        assert_eq!(fetches.load(Ordering::SeqCst), 1);
        assert_eq!(second, first);
    }

    #[tokio::test]
    async fn window_cache_refetches_stale_window() {
        let clock = test_clock();
        let cache = WindowCache::with_clock(Duration::from_secs(5), clock.now_fn());
        let key = WindowCacheKey::new("app.example", "/var/log/app.log", 10);
        let fetches = Arc::new(AtomicUsize::new(0));

        cache
            .get_or_fetch(key.clone(), {
                let fetches = Arc::clone(&fetches);
                || async move {
                    fetches.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, LensError>(sample_window("ERROR release_id=43301 first", 33))
                }
            })
            .await
            .expect("first fetch");
        clock.advance(Duration::from_secs(6));
        let second = cache
            .get_or_fetch(key, {
                let fetches = Arc::clone(&fetches);
                || async move {
                    fetches.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, LensError>(sample_window("ERROR release_id=43301 second", 34))
                }
            })
            .await
            .expect("stale fetch");

        assert_eq!(fetches.load(Ordering::SeqCst), 2);
        assert_eq!(second.lines, vec!["ERROR release_id=43301 second"]);
    }

    #[tokio::test]
    async fn window_cache_isolates_distinct_keys() {
        let clock = test_clock();
        let cache = WindowCache::with_clock(Duration::from_secs(5), clock.now_fn());
        let first_key = WindowCacheKey::new("app.example", "/var/log/app.log", 10);
        let second_key = WindowCacheKey::new("app.example", "/var/log/audit.log", 10);
        let fetches = Arc::new(AtomicUsize::new(0));

        let first = cache
            .get_or_fetch(first_key, {
                let fetches = Arc::clone(&fetches);
                || async move {
                    fetches.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, LensError>(sample_window("ERROR release_id=43301 app", 30))
                }
            })
            .await
            .expect("first key fetch");
        let second = cache
            .get_or_fetch(second_key, {
                let fetches = Arc::clone(&fetches);
                || async move {
                    fetches.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, LensError>(sample_window("ERROR release_id=43301 audit", 32))
                }
            })
            .await
            .expect("second key fetch");

        assert_eq!(fetches.load(Ordering::SeqCst), 2);
        assert_ne!(second, first);
    }

    #[tokio::test]
    async fn grep_uses_prewarmed_window_cache_without_fetch() {
        let source = test_source();
        let window = SshLogOutput {
            lines: vec![
                "ERROR release_id=43301 first".to_string(),
                "INFO release_id=43301 ignored".to_string(),
                "ERROR release_id=43301 second".to_string(),
            ],
            truncated_at: vec![TruncatedAt::Bytes],
            bytes: 91,
            metadata: None,
        };
        let re = regex::Regex::new("43301").expect("regex");
        let level_re = regex::Regex::new(r"(?i)\bERROR\b").expect("level regex");
        let expected = source.filter_grep_output(
            "43301",
            Some("ERROR"),
            1,
            window.clone(),
            &re,
            Some(&level_re),
        );
        source.grep_window_cache.store(
            source.grep_window_cache_key(BOUNDED_TAIL_FOR_GREP),
            window,
            Instant::now(),
        );
        let fetches = Arc::new(AtomicUsize::new(0));

        let actual = source
            .grep_with_tail_fetcher("43301", Some("ERROR"), 1, {
                let fetches = Arc::clone(&fetches);
                || async move {
                    fetches.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, LensError>(sample_window("ERROR fetched unexpectedly", 26))
                }
            })
            .await
            .expect("cached grep");

        assert_eq!(fetches.load(Ordering::SeqCst), 0);
        assert_eq!(actual, expected);
    }

    #[tokio::test]
    async fn grep_normalizes_empty_level_to_absent() {
        let source = test_source();

        let output = source
            .grep_with_tail_fetcher("43301", Some(""), 1, || async {
                Ok::<_, LensError>(SshLogOutput {
                    lines: vec![
                        "INFO release_id=43301 booted".to_string(),
                        "ERROR release_id=43301 failed".to_string(),
                    ],
                    truncated_at: Vec::new(),
                    bytes: 58,
                    metadata: None,
                })
            })
            .await
            .expect("grep");

        assert_eq!(output.lines, vec!["INFO release_id=43301 booted"]);
        let metadata = output.metadata.as_ref().expect("metadata");
        assert_eq!(metadata.level, None);
        assert_eq!(metadata.matched_lines, 2);
        assert_eq!(metadata.returned_lines, 1);
        assert_eq!(metadata.truncated_at, vec![TruncatedAt::Rows]);
    }

    #[tokio::test]
    async fn cached_grep_preserves_tail_truncation_metadata() {
        let source = test_source();
        source.grep_window_cache.store(
            source.grep_window_cache_key(BOUNDED_TAIL_FOR_GREP),
            SshLogOutput {
                lines: vec![
                    "ERROR release_id=43301 first".to_string(),
                    "ERROR release_id=43301 second".to_string(),
                ],
                truncated_at: vec![TruncatedAt::Bytes, TruncatedAt::LineBytes],
                bytes: 4096,
                metadata: None,
            },
            Instant::now(),
        );
        let fetches = Arc::new(AtomicUsize::new(0));

        let output = source
            .grep_with_tail_fetcher("43301", None, 1, {
                let fetches = Arc::clone(&fetches);
                || async move {
                    fetches.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, LensError>(sample_window("ERROR fetched unexpectedly", 26))
                }
            })
            .await
            .expect("cached grep");

        assert_eq!(fetches.load(Ordering::SeqCst), 0);
        assert_eq!(
            output.truncated_at,
            vec![
                TruncatedAt::Bytes,
                TruncatedAt::LineBytes,
                TruncatedAt::Rows
            ]
        );
        let metadata = output.metadata.as_ref().expect("metadata");
        assert_eq!(metadata.searched_bytes, 4096);
        assert_eq!(metadata.tail_window_lines, BOUNDED_TAIL_FOR_GREP);
        assert_eq!(metadata.matched_lines, 2);
        assert_eq!(metadata.returned_lines, 1);
        assert_eq!(metadata.truncated_at, output.truncated_at);
    }

    #[tokio::test]
    async fn grep_window_uses_fresh_window_cache_without_fetch() {
        let source = test_source();
        let fetches = Arc::new(AtomicUsize::new(0));

        let first = source
            .grep_window_with_tail_fetcher("43301", None, 100, false, {
                let fetches = Arc::clone(&fetches);
                || async move {
                    fetches.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, LensError>(sample_window("ERROR release_id=43301 first", 30))
                }
            })
            .await
            .expect("first window");
        let second = source
            .grep_window_with_tail_fetcher("43301", None, 100, false, {
                let fetches = Arc::clone(&fetches);
                || async move {
                    fetches.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, LensError>(sample_window("ERROR release_id=43301 second", 31))
                }
            })
            .await
            .expect("cached window");

        assert_eq!(fetches.load(Ordering::SeqCst), 1);
        assert_eq!(second.lines, first.lines);
    }

    #[tokio::test]
    async fn grep_window_refresh_refetches_fresh_window_cache() {
        let source = test_source();
        let fetches = Arc::new(AtomicUsize::new(0));

        source
            .grep_window_with_tail_fetcher("43301", None, 100, false, {
                let fetches = Arc::clone(&fetches);
                || async move {
                    fetches.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, LensError>(sample_window("ERROR release_id=43301 first", 30))
                }
            })
            .await
            .expect("first window");
        let refreshed = source
            .grep_window_with_tail_fetcher("43301", None, 100, true, {
                let fetches = Arc::clone(&fetches);
                || async move {
                    fetches.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, LensError>(sample_window("ERROR release_id=43301 refreshed", 34))
                }
            })
            .await
            .expect("refreshed window");

        assert_eq!(fetches.load(Ordering::SeqCst), 2);
        assert_eq!(refreshed.lines, vec!["ERROR release_id=43301 refreshed"]);
    }

    #[test]
    fn connect_timeout_stderr_is_classified() {
        assert!(ssh_stderr_indicates_connect_timeout(
            b"ssh: connect to host app.example port 22: Connection timed out"
        ));
    }

    #[test]
    fn refused_connect_stderr_is_not_classified_as_timeout() {
        assert!(!ssh_stderr_indicates_connect_timeout(
            b"ssh: connect to host app.example port 22: Connection refused"
        ));
    }

    #[test]
    fn timeout_error_reports_ssh_context() {
        let err = timeout_error(
            "prod-logs",
            "ssh log command/read",
            "log_tail",
            "app.example",
            "/var/log/app.log",
            Duration::from_secs(30),
        );

        let msg = sanitize_error(&err);
        assert!(msg.contains("phase=ssh log command/read"), "{msg}");
        assert!(msg.contains("operation=log_tail"), "{msg}");
        assert!(msg.contains("profile=prod-logs"), "{msg}");
        assert!(msg.contains("host=app.example"), "{msg}");
        assert!(msg.contains("path=/var/log/app.log"), "{msg}");
    }

    #[test]
    fn empty_grep_renders_actionable_metadata() {
        let source = test_source();
        let re = regex::Regex::new("43301").expect("regex");
        let level_re = regex::Regex::new("ERROR").expect("level regex");

        let output = source.filter_grep_output(
            "43301",
            Some("ERROR"),
            100,
            SshLogOutput {
                lines: vec!["INFO worker booted".to_string()],
                truncated_at: Vec::new(),
                bytes: 18,
                metadata: None,
            },
            &re,
            Some(&level_re),
        );

        assert!(output.lines.is_empty());
        assert_eq!(
            output.metadata.as_ref().expect("metadata").status,
            "no_matches"
        );
        let text = output.into_text();
        assert!(text.contains("\"operation\":\"log_grep\""), "{text}");
        assert!(text.contains("\"status\":\"no_matches\""), "{text}");
        assert!(text.contains("\"profile\":\"prod-logs\""), "{text}");
        assert!(text.contains("\"host\":\"app.example\""), "{text}");
        assert!(text.contains("\"path\":\"/var/log/app.log\""), "{text}");
        assert!(text.contains("\"pattern\":\"43301\""), "{text}");
        assert!(text.contains("\"level\":\"ERROR\""), "{text}");
        assert!(text.contains("\"tail_window_lines\":10000"), "{text}");
        assert!(text.contains("\"searched_lines\":1"), "{text}");
        assert!(text.contains("\"matched_lines\":0"), "{text}");
        assert!(text.contains("\"searched_bytes\":18"), "{text}");
    }

    #[test]
    fn matched_grep_renders_metadata_before_matches() {
        let source = test_source();
        let re = regex::Regex::new("43301").expect("regex");

        let output = source.filter_grep_output(
            "43301",
            None,
            100,
            SshLogOutput {
                lines: vec!["ERROR release_id=43301 first".to_string()],
                truncated_at: Vec::new(),
                bytes: 29,
                metadata: None,
            },
            &re,
            None,
        );

        assert_eq!(output.lines, vec!["ERROR release_id=43301 first"]);
        assert!(output.truncated_at.is_empty());
        let metadata = output.metadata.as_ref().expect("metadata");
        assert_eq!(metadata.status, "matches");
        assert_eq!(metadata.matched_lines, 1);
        assert_eq!(metadata.returned_lines, 1);
        let text = output.into_text();
        let mut lines = text.lines();
        let metadata = lines.next().expect("metadata line");
        assert!(metadata.contains("\"status\":\"matches\""), "{metadata}");
        assert!(metadata.contains("\"matched_lines\":1"), "{metadata}");
        assert!(metadata.contains("\"returned_lines\":1"), "{metadata}");
        assert_eq!(lines.next(), Some("ERROR release_id=43301 first"));
        assert_eq!(lines.next(), None);
    }

    #[test]
    fn limit_truncated_grep_renders_metadata_before_matches() {
        let source = test_source();
        let re = regex::Regex::new("43301").expect("regex");

        let output = source.filter_grep_output(
            "43301",
            None,
            1,
            SshLogOutput {
                lines: vec![
                    "ERROR release_id=43301 first".to_string(),
                    "ERROR release_id=43301 second".to_string(),
                ],
                truncated_at: Vec::new(),
                bytes: 58,
                metadata: None,
            },
            &re,
            None,
        );

        assert_eq!(output.lines, vec!["ERROR release_id=43301 first"]);
        assert_eq!(output.truncated_at, vec![TruncatedAt::Rows]);
        let text = output.into_text();
        let mut lines = text.lines();
        let metadata = lines.next().expect("metadata line");
        assert!(metadata.contains("\"status\":\"truncated\""), "{metadata}");
        assert!(metadata.contains("\"matched_lines\":2"), "{metadata}");
        assert!(metadata.contains("\"returned_lines\":1"), "{metadata}");
        assert!(
            metadata.contains("\"truncated_at\":[\"Rows\"]"),
            "{metadata}"
        );
        assert_eq!(lines.next(), Some("ERROR release_id=43301 first"));
        assert_eq!(lines.next(), None);
    }

    fn test_source() -> SshLogSource {
        SshLogSource::new(
            "prod-logs",
            "app.example",
            "/var/log/app.log",
            SshLogCaps {
                line_bytes: 1024,
                bytes: 4096,
                timeout: Duration::from_secs(1),
            },
        )
        .expect("source")
    }

    fn sample_window(line: &str, bytes: usize) -> SshLogOutput {
        SshLogOutput {
            lines: vec![line.to_string()],
            truncated_at: Vec::new(),
            bytes,
            metadata: None,
        }
    }

    fn test_clock() -> TestClock {
        TestClock {
            now: Arc::new(Mutex::new(Instant::now())),
        }
    }

    #[derive(Clone)]
    struct TestClock {
        now: Arc<Mutex<Instant>>,
    }

    impl TestClock {
        fn now_fn(&self) -> impl Fn() -> Instant + Send + Sync + 'static {
            let now = Arc::clone(&self.now);
            move || *now.lock().expect("test clock")
        }

        fn advance(&self, duration: Duration) {
            let mut now = self.now.lock().expect("test clock");
            *now += duration;
        }
    }
}
