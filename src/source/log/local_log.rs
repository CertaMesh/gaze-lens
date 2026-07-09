use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use tokio::io::AsyncSeekExt;

use crate::errors::LensError;
use crate::session::TruncatedAt;
use crate::source::log::ssh_log::{
    BOUNDED_TAIL_FOR_GREP, HARD_CAP_LINES, read_capped, split_and_cap_lines_with_truncation,
};
use crate::value::LowerError;

const READ_CHUNK_BYTES: usize = 64 * 1024;
const GREP_WINDOW_CACHE_TTL: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Copy)]
pub struct LocalLogCaps {
    pub line_bytes: usize,
    pub bytes: usize,
    pub timeout: Duration,
}

#[derive(Debug)]
pub struct LocalLogSource {
    profile_name: String,
    path: PathBuf,
    max_line_bytes: usize,
    max_total_bytes: usize,
    timeout: Duration,
    grep_window_cache: WindowCache,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LocalLogOutput {
    pub lines: Vec<String>,
    pub truncated_at: Vec<TruncatedAt>,
    pub bytes: usize,
    pub metadata: Option<LocalLogMetadata>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct LocalLogMetadata {
    pub operation: &'static str,
    pub status: &'static str,
    pub profile: String,
    pub source_kind: &'static str,
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
    path: PathBuf,
    window_lines: usize,
}

#[derive(Debug, Clone)]
struct CachedWindow {
    minted_at: Instant,
    output: LocalLogOutput,
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
    ) -> Result<LocalLogOutput, LensError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<LocalLogOutput, LensError>>,
    {
        let now = (self.now)();
        if let Some(output) = self.fresh(&key, now)? {
            return Ok(output);
        }

        let output = fetch().await?;
        self.store(key, output.clone(), (self.now)())?;
        Ok(output)
    }

    async fn get_or_fetch_refresh<F, Fut>(
        &self,
        key: WindowCacheKey,
        refresh: bool,
        fetch: F,
    ) -> Result<LocalLogOutput, LensError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<LocalLogOutput, LensError>>,
    {
        if !refresh {
            return self.get_or_fetch(key, fetch).await;
        }

        let output = fetch().await?;
        self.store(key, output.clone(), (self.now)())?;
        Ok(output)
    }

    fn fresh(
        &self,
        key: &WindowCacheKey,
        now: Instant,
    ) -> Result<Option<LocalLogOutput>, LensError> {
        Ok(self
            .entries()?
            .get(key)
            .filter(|entry| now.saturating_duration_since(entry.minted_at) <= self.ttl)
            .map(|entry| entry.output.clone()))
    }

    fn store(
        &self,
        key: WindowCacheKey,
        output: LocalLogOutput,
        minted_at: Instant,
    ) -> Result<(), LensError> {
        self.entries()?
            .insert(key, CachedWindow { minted_at, output });
        Ok(())
    }

    fn entries(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, HashMap<WindowCacheKey, CachedWindow>>, LensError> {
        self.entries.lock().map_err(|_| LensError::Internal {
            detail: "local log window cache lock poisoned".to_string(),
        })
    }
}

impl LocalLogOutput {
    pub fn into_text(self) -> Result<String, LensError> {
        let lines = self.lines.join("\n");
        let Some(metadata) = self.metadata else {
            return Ok(lines);
        };
        let metadata = serde_json::to_string(&metadata).map_err(|err| {
            source_error(
                &metadata.profile,
                format!("local log metadata serialization failed: {err}"),
            )
        })?;
        Ok(if lines.is_empty() {
            metadata
        } else {
            format!("{metadata}\n{lines}")
        })
    }
}

impl LocalLogSource {
    pub fn new(
        profile_name: impl Into<String>,
        path: impl Into<PathBuf>,
        caps: LocalLogCaps,
    ) -> Result<Self, LensError> {
        let profile_name = profile_name.into();
        let path = path.into();
        if path.as_os_str().is_empty() {
            return Err(source_error(
                &profile_name,
                "local_log path must not be empty".to_string(),
            ));
        }
        Ok(Self {
            profile_name,
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

    pub async fn tail(&self, lines: usize) -> Result<LocalLogOutput, LensError> {
        self.tail_for_operation(lines, "log_tail").await
    }

    async fn tail_for_operation(
        &self,
        lines: usize,
        operation: &str,
    ) -> Result<LocalLogOutput, LensError> {
        tokio::time::timeout(self.timeout, self.tail_inner(lines))
            .await
            .map_err(|_| LensError::OperationTimeout {
                phase: "local log read".to_string(),
                operation: operation.to_string(),
                timeout_secs: self.timeout.as_secs(),
                context: Some(format!(
                    "profile={} path={}",
                    self.profile_name,
                    self.path.display()
                )),
            })?
    }

    pub async fn grep(
        &self,
        pattern: &str,
        level: Option<&str>,
        limit: usize,
    ) -> Result<LocalLogOutput, LensError> {
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
    ) -> Result<LocalLogOutput, LensError> {
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
    ) -> Result<LocalLogOutput, LensError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<LocalLogOutput, LensError>>,
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
    ) -> Result<LocalLogOutput, LensError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<LocalLogOutput, LensError>>,
    {
        let level = non_empty_level(level);
        let re = regex::Regex::new(pattern)
            .map_err(|_| source_error(&self.profile_name, "invalid log grep regex".to_string()))?;
        let level_re = level
            .map(|level| regex::Regex::new(&format!(r"(?i)\b{}\b", regex::escape(level))))
            .transpose()
            .map_err(|_| {
                source_error(&self.profile_name, "invalid log level filter".to_string())
            })?;
        let output = self
            .grep_window_cache
            .get_or_fetch(self.grep_window_cache_key(BOUNDED_TAIL_FOR_GREP), fetch)
            .await?;
        Ok(self.filter_grep_output(pattern, level, limit, output, &re, level_re.as_ref()))
    }

    fn grep_window_cache_key(&self, window_lines: usize) -> WindowCacheKey {
        WindowCacheKey {
            path: self.path.clone(),
            window_lines,
        }
    }

    fn filter_grep_output(
        &self,
        pattern: &str,
        level: Option<&str>,
        limit: usize,
        output: LocalLogOutput,
        re: &regex::Regex,
        level_re: Option<&regex::Regex>,
    ) -> LocalLogOutput {
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
            LocalLogMetadata {
                operation: "log_grep",
                status,
                profile: self.profile_name.clone(),
                source_kind: "local_log",
                path: self.path.to_string_lossy().into_owned(),
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
        LocalLogOutput {
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
        output: LocalLogOutput,
    ) -> LocalLogOutput {
        let level = non_empty_level(level);
        let searched_lines = output.lines.len();
        let searched_bytes = output.bytes;
        let truncated_at = output.truncated_at;
        let metadata = Some(LocalLogMetadata {
            operation: "log_grep",
            status: grep_status(searched_lines, &truncated_at),
            profile: self.profile_name.clone(),
            source_kind: "local_log",
            path: self.path.to_string_lossy().into_owned(),
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
        LocalLogOutput {
            lines: output.lines,
            truncated_at,
            bytes: searched_bytes,
            metadata,
        }
    }

    async fn tail_inner(&self, lines: usize) -> Result<LocalLogOutput, LensError> {
        let capped_lines = lines.min(HARD_CAP_LINES);
        let mut raw = self.read_tail_bytes(capped_lines).await?;
        let mut truncated_at = Vec::new();
        if raw.len() > self.max_total_bytes {
            truncated_at.push(TruncatedAt::Bytes);
            raw.truncate(self.max_total_bytes);
        }
        validate_utf8(&raw)?;
        let (lines, line_truncated) =
            split_and_cap_lines_with_truncation(&raw, self.max_line_bytes);
        if line_truncated {
            truncated_at.push(TruncatedAt::LineBytes);
        }
        Ok(LocalLogOutput {
            lines,
            truncated_at,
            bytes: raw.len(),
            metadata: None,
        })
    }

    async fn read_tail_bytes(&self, lines: usize) -> Result<Vec<u8>, LensError> {
        if lines == 0 {
            return Ok(Vec::new());
        }
        let mut file = tokio::fs::File::open(&self.path)
            .await
            .map_err(|err| source_error(&self.profile_name, err.to_string()))?;
        let len = file
            .metadata()
            .await
            .map_err(|err| source_error(&self.profile_name, err.to_string()))?
            .len();
        if len == 0 {
            return Ok(Vec::new());
        }

        let max_read = self.max_total_bytes.saturating_add(1);
        let start = self.tail_start_offset(&mut file, len, lines).await?;
        file.seek(std::io::SeekFrom::Start(start))
            .await
            .map_err(|err| source_error(&self.profile_name, err.to_string()))?;
        read_capped(&mut file, max_read)
            .await
            .map_err(|err| source_error(&self.profile_name, err.to_string()))
    }

    async fn tail_start_offset(
        &self,
        file: &mut tokio::fs::File,
        len: u64,
        lines: usize,
    ) -> Result<u64, LensError> {
        let last_byte_pos = len.saturating_sub(1);
        file.seek(std::io::SeekFrom::Start(last_byte_pos))
            .await
            .map_err(|err| source_error(&self.profile_name, err.to_string()))?;
        let last_byte = read_capped(&mut *file, 1)
            .await
            .map_err(|err| source_error(&self.profile_name, err.to_string()))?;
        let target_newlines = lines.saturating_add(usize::from(last_byte == b"\n"));
        let mut pos = len;
        let mut seen = 0usize;
        while pos > 0 {
            let read_len =
                usize::try_from(pos.min(READ_CHUNK_BYTES as u64)).unwrap_or(READ_CHUNK_BYTES);
            let start = pos.saturating_sub(read_len as u64);
            file.seek(std::io::SeekFrom::Start(start))
                .await
                .map_err(|err| source_error(&self.profile_name, err.to_string()))?;
            let chunk = read_capped(&mut *file, read_len)
                .await
                .map_err(|err| source_error(&self.profile_name, err.to_string()))?;
            for index in (0..chunk.len()).rev() {
                if chunk[index] == b'\n' {
                    seen += 1;
                    if seen == target_newlines {
                        return Ok(start + index as u64 + 1);
                    }
                }
            }
            pos = start;
        }
        Ok(0)
    }
}

fn validate_utf8(raw: &[u8]) -> Result<(), LensError> {
    std::str::from_utf8(raw).map(|_| ()).map_err(|err| {
        LensError::ConvertError(LowerError::Decode {
            kind: "local_log",
            detail: err.to_string(),
        })
    })
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

fn source_error(profile_name: &str, detail: String) -> LensError {
    LensError::SourceError {
        source_name: profile_name.to_string(),
        detail,
        sql: None,
        stderr: None,
    }
}

#[cfg(test)]
mod tests {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::path::PathBuf;
    use std::time::Duration;

    use crate::errors::LensError;
    use crate::value::LowerError;

    use super::{
        LocalLogCaps, LocalLogOutput, LocalLogSource, READ_CHUNK_BYTES, WindowCache, WindowCacheKey,
    };

    #[tokio::test]
    async fn tail_reads_last_lines_from_local_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("app.log");
        tokio::fs::write(
            &path,
            "INFO booted\nERROR first@example.com failed\nWARN recovered\n",
        )
        .await
        .expect("write log");
        let source = LocalLogSource::new(
            "dev-log",
            path.clone(),
            LocalLogCaps {
                line_bytes: 1024,
                bytes: 4096,
                timeout: Duration::from_secs(1),
            },
        )
        .expect("source");

        let output = source.tail(2).await.expect("tail");

        assert_eq!(
            output.lines,
            vec![
                "ERROR first@example.com failed".to_string(),
                "WARN recovered".to_string()
            ]
        );
        assert!(output.truncated_at.is_empty());
    }

    #[tokio::test]
    async fn tail_one_line_matches_ssh_tail_n_for_trailing_blank_line() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("app.log");
        tokio::fs::write(&path, "ERROR previous@example.com leaked\n\n")
            .await
            .expect("write log");
        let source = LocalLogSource::new(
            "dev-log",
            path,
            LocalLogCaps {
                line_bytes: 1024,
                bytes: 4096,
                timeout: Duration::from_secs(1),
            },
        )
        .expect("source");

        let output = source.tail(1).await.expect("tail");

        assert!(output.lines.is_empty());
        assert_eq!(output.bytes, 1);
        assert!(output.truncated_at.is_empty());
    }

    #[tokio::test]
    async fn tail_byte_cap_matches_ssh_when_requested_lines_exceed_file_lines() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("app.log");
        let prefix = "A".repeat(READ_CHUNK_BYTES + 32);
        let contents = format!("{prefix}\nERROR newest@example.com hidden\n");
        tokio::fs::write(&path, contents).await.expect("write log");
        let source = LocalLogSource::new(
            "dev-log",
            path,
            LocalLogCaps {
                line_bytes: 1024,
                bytes: 16,
                timeout: Duration::from_secs(1),
            },
        )
        .expect("source");

        let output = source.tail(10).await.expect("tail");

        assert_eq!(output.lines, vec!["A".repeat(16)]);
        assert_eq!(output.bytes, 16);
        assert_eq!(
            output.truncated_at,
            vec![crate::session::TruncatedAt::Bytes]
        );
    }

    #[tokio::test]
    async fn window_cache_lock_poison_returns_internal_error() {
        let cache = WindowCache::new(Duration::from_secs(5));
        let key = WindowCacheKey {
            path: PathBuf::from("/tmp/app.log"),
            window_lines: 10,
        };
        let poison = catch_unwind(AssertUnwindSafe(|| {
            let _guard = cache.entries.lock().expect("lock");
            panic!("poison local log cache");
        }));
        assert!(poison.is_err());

        let err = cache
            .get_or_fetch(key, || async {
                Ok::<_, LensError>(LocalLogOutput {
                    lines: vec!["ERROR fetched unexpectedly".to_string()],
                    truncated_at: Vec::new(),
                    bytes: 26,
                    metadata: None,
                })
            })
            .await
            .expect_err("poison error");

        assert!(matches!(err, LensError::Internal { .. }));
    }

    #[tokio::test]
    async fn grep_filters_local_tail_window_and_reports_row_truncation() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("app.log");
        tokio::fs::write(
            &path,
            "INFO booted\nERROR first@example.com failed\nERROR second@example.com failed\n",
        )
        .await
        .expect("write log");
        let source = LocalLogSource::new(
            "dev-log",
            path,
            LocalLogCaps {
                line_bytes: 1024,
                bytes: 4096,
                timeout: Duration::from_secs(1),
            },
        )
        .expect("source");

        let output = source
            .grep(r"ERROR .*@example\.com", None, 1)
            .await
            .expect("grep");

        assert_eq!(output.lines, vec!["ERROR first@example.com failed"]);
        assert_eq!(output.truncated_at, vec![crate::session::TruncatedAt::Rows]);
        let text = output.into_text().expect("text");
        let metadata = text.lines().next().expect("metadata");
        assert!(
            metadata.contains(r#""source_kind":"local_log""#),
            "{metadata}"
        );
        assert!(metadata.contains(r#""path":""#), "{metadata}");
        assert!(metadata.contains(r#""matched_lines":2"#), "{metadata}");
        assert!(metadata.contains(r#""returned_lines":1"#), "{metadata}");
        assert_eq!(text.lines().nth(1), Some("ERROR first@example.com failed"));
    }

    #[tokio::test]
    async fn matched_grep_renders_local_metadata_before_matches() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("app.log");
        tokio::fs::write(&path, "INFO booted\nERROR release_id=43301 failed\n")
            .await
            .expect("write log");
        let source = LocalLogSource::new(
            "dev-log",
            path,
            LocalLogCaps {
                line_bytes: 1024,
                bytes: 4096,
                timeout: Duration::from_secs(1),
            },
        )
        .expect("source");

        let output = source.grep("43301", None, 100).await.expect("grep");

        assert_eq!(output.lines, vec!["ERROR release_id=43301 failed"]);
        assert!(output.truncated_at.is_empty());
        let metadata = output.metadata.as_ref().expect("metadata");
        assert_eq!(metadata.status, "matches");
        assert_eq!(metadata.matched_lines, 1);
        assert_eq!(metadata.returned_lines, 1);
        let text = output.into_text().expect("text");
        let mut lines = text.lines();
        let metadata = lines.next().expect("metadata");
        assert!(
            metadata.contains(r#""source_kind":"local_log""#),
            "{metadata}"
        );
        assert!(metadata.contains(r#""status":"matches""#), "{metadata}");
        assert!(metadata.contains(r#""matched_lines":1"#), "{metadata}");
        assert_eq!(lines.next(), Some("ERROR release_id=43301 failed"));
        assert_eq!(lines.next(), None);
    }

    #[tokio::test]
    async fn tail_enforces_byte_and_line_caps() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("app.log");
        tokio::fs::write(&path, "1234567890\nabcdefghi\n")
            .await
            .expect("write log");
        let source = LocalLogSource::new(
            "dev-log",
            path,
            LocalLogCaps {
                line_bytes: 5,
                bytes: 12,
                timeout: Duration::from_secs(1),
            },
        )
        .expect("source");

        let output = source.tail(10).await.expect("tail");

        assert_eq!(output.lines, vec!["12345", "a"]);
        assert_eq!(
            output.truncated_at,
            vec![
                crate::session::TruncatedAt::Bytes,
                crate::session::TruncatedAt::LineBytes
            ]
        );
    }

    #[tokio::test]
    async fn tail_rejects_invalid_utf8() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("app.log");
        tokio::fs::write(&path, [0xff, b'\n'])
            .await
            .expect("write log");
        let source = LocalLogSource::new(
            "dev-log",
            path,
            LocalLogCaps {
                line_bytes: 1024,
                bytes: 4096,
                timeout: Duration::from_secs(1),
            },
        )
        .expect("source");

        let err = source.tail(10).await.expect_err("invalid utf8");

        assert!(matches!(
            err,
            LensError::ConvertError(LowerError::Decode {
                kind: "local_log",
                ..
            })
        ));
    }
}
