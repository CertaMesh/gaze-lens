use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::future::Future;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use gaze_mcp_core::{Tool, ToolCtx, ToolDescriptor, ToolError, ToolResponse};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::errors::LensError;
use crate::session::{CleanOutput, ResultSummary, Session, TruncatedAt};

use super::{clean_output_response, invoke_session_tool, lens_error_to_tool_error, schema_for};

const KEYWORD_INDEX_CACHE_TTL: Duration = Duration::from_secs(3);

static KEYWORD_INDEX_CACHE: OnceLock<KeywordIndexCache> = OnceLock::new();

tokio::task_local! {
    pub(crate) static RAW_LOG_GREP_PATTERN: Option<String>;
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct LogGrepArgs {
    #[schemars(
        description = "Configured profile name selecting the source to dispatch. Required. Pattern: ^[a-z0-9][a-z0-9_-]{0,63}$.",
        regex(pattern = r"^[a-z0-9][a-z0-9_-]{0,63}$")
    )]
    pub profile: String,
    #[schemars(
        description = "Search expression. In regex mode (default), this is a Rust regex matched over RAW log text before displayed lines are redacted, so it can act as a raw-text presence/absence oracle. In keyword mode, this is split into literal terms and AND-matched over redacted log text; token queries must use the complete `<hash:Name_N>` token minted for the current session, because partial fragments such as `Email_1` intentionally return 0 hits."
    )]
    pub pattern: String,
    #[serde(default)]
    pub level: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    #[schemars(
        description = "Search mode: `regex` (default) treats pattern as a Rust regex over RAW log text before displayed lines are redacted; `keyword` treats pattern as literal terms matched over redacted log text. In keyword mode, token searches require the complete `<hash:Name_N>` token."
    )]
    pub mode: Option<String>,
    #[serde(default)]
    pub refresh: Option<bool>,
}

pub struct LogGrepTool {
    session: Arc<Session>,
    descriptor: ToolDescriptor,
}

impl LogGrepTool {
    pub fn new(session: Arc<Session>) -> Self {
        Self {
            session,
            descriptor: ToolDescriptor::agent("log_grep", schema_for::<LogGrepArgs>())
                .with_description(
                    "Search a configured SSH log source. mode=regex (default) evaluates the \
                     pattern over RAW log text (only displayed lines are redacted), so a regex \
                     can act as a presence/absence oracle for raw PII; prefer mode=keyword \
                     (matches over redacted text) for sensitive or production logs.",
                ),
        }
    }
}

#[async_trait]
impl Tool for LogGrepTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    async fn invoke(&self, ctx: &ToolCtx<'_>) -> Result<ToolResponse, ToolError> {
        let args = ctx.redacted_args();
        let mode = log_grep_mode(args)?;
        let profile = profile_key_from_args(args);
        warn_if_production_regex_mode(profile, self.session.profile_is_production(profile), mode);

        match mode {
            LogGrepMode::Regex => invoke_session_tool(&self.session, "log_grep", ctx).await,
            LogGrepMode::Keyword => self.invoke_keyword(ctx).await,
        }
    }
}

impl LogGrepTool {
    async fn invoke_keyword(&self, ctx: &ToolCtx<'_>) -> Result<ToolResponse, ToolError> {
        let raw_pattern = RAW_LOG_GREP_PATTERN
            .try_with(Clone::clone)
            .map_err(|_| {
                ToolError::internal(LensError::Internal {
                    detail: "keyword log_grep raw pattern task-local was not scoped".to_string(),
                })
            })?
            .ok_or_else(|| ToolError::InvalidArgs("log_grep `pattern` is required".to_string()))?;
        let request = keyword_request_from_args(ctx.redacted_args(), raw_pattern)?;
        let key = keyword_cache_key(ctx, &request);
        let lookup = keyword_index_cache()
            .get_or_fetch(key, request.refresh, || async {
                let clean = self
                    .session
                    .invoke_core_tool("log_grep", ctx.call_id(), ctx.redacted_args().clone())
                    .await
                    .map_err(lens_error_to_tool_error)?;
                redacted_keyword_window_from_clean(clean)
            })
            .await?;
        let clean =
            filter_keyword_indexed_window(&lookup.indexed.window, &lookup.indexed.index, &request)?;
        if lookup.cache_hit {
            self.session.record_core_summary(
                ctx.call_id(),
                keyword_core_summary(&lookup.indexed.window, &request),
            );
        }
        clean_output_response(clean)
    }
}

fn keyword_index_cache() -> &'static KeywordIndexCache {
    KEYWORD_INDEX_CACHE.get_or_init(|| KeywordIndexCache::new(KEYWORD_INDEX_CACHE_TTL))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LogGrepMode {
    Regex,
    Keyword,
}

fn log_grep_mode(args: &serde_json::Value) -> Result<LogGrepMode, ToolError> {
    match args.get("mode") {
        None | Some(serde_json::Value::Null) => Ok(LogGrepMode::Regex),
        Some(serde_json::Value::String(mode)) if mode == "regex" => Ok(LogGrepMode::Regex),
        Some(serde_json::Value::String(mode)) if mode == "keyword" => Ok(LogGrepMode::Keyword),
        Some(serde_json::Value::String(mode)) => Err(ToolError::InvalidArgs(format!(
            "invalid log_grep mode `{mode}`; expected `regex` or `keyword`"
        ))),
        Some(_) => Err(ToolError::InvalidArgs(
            "invalid log_grep mode; expected `regex` or `keyword`".to_string(),
        )),
    }
}

fn profile_key_from_args(args: &serde_json::Value) -> &str {
    args.get("profile")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
}

fn warn_if_production_regex_mode(profile: &str, production: bool, mode: LogGrepMode) {
    if production && mode == LogGrepMode::Regex {
        tracing::warn!(
            target: "gaze_lens::mcp::tools::log_grep",
            profile,
            mode = "regex",
            "production log_grep regex mode can act as a raw-text presence/absence oracle; use mode=\"keyword\" for production logs"
        );
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct KeywordRequest {
    pattern: String,
    match_pattern: String,
    level: Option<String>,
    limit: usize,
    refresh: bool,
    profile_key: String,
}

#[derive(Debug, Clone, PartialEq)]
struct RedactedKeywordWindow {
    lines: Vec<String>,
    metadata: Option<serde_json::Value>,
    truncated_at: Vec<TruncatedAt>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct KeywordIndex {
    postings: BTreeMap<String, Vec<usize>>,
}

impl KeywordIndex {
    fn build(lines: &[String]) -> Self {
        let mut postings: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (line_index, line) in lines.iter().enumerate() {
            for term in indexed_line_terms(line) {
                postings.entry(term).or_default().push(line_index);
            }
        }
        Self { postings }
    }

    fn matching_lines(&self, terms: &[String]) -> Vec<usize> {
        let Some((first, rest)) = terms.split_first() else {
            return Vec::new();
        };
        let mut matches = self.postings.get(first).cloned().unwrap_or_default();
        for term in rest {
            let postings = self.postings.get(term).map(Vec::as_slice).unwrap_or(&[]);
            matches = intersect_sorted(&matches, postings);
            if matches.is_empty() {
                break;
            }
        }
        matches
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct KeywordCacheKey {
    session_id: String,
    profile_key: String,
}

impl KeywordCacheKey {
    fn new(session_id: impl Into<String>, profile_key: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            profile_key: profile_key.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct IndexedKeywordWindow {
    window: RedactedKeywordWindow,
    index: KeywordIndex,
}

impl IndexedKeywordWindow {
    fn new(window: RedactedKeywordWindow) -> Self {
        let index = KeywordIndex::build(&window.lines);
        Self { window, index }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct KeywordIndexLookup {
    indexed: IndexedKeywordWindow,
    cache_hit: bool,
}

#[derive(Debug, Clone)]
struct CachedKeywordWindow {
    minted_at: Instant,
    indexed: IndexedKeywordWindow,
}

struct KeywordIndexCache {
    ttl: Duration,
    now: Box<dyn Fn() -> Instant + Send + Sync>,
    entries: Mutex<HashMap<KeywordCacheKey, CachedKeywordWindow>>,
}

impl KeywordIndexCache {
    fn new(ttl: Duration) -> Self {
        Self::with_clock(ttl, Instant::now)
    }

    fn with_clock(
        ttl: Duration,
        now: impl Fn() -> Instant + Send + Sync + 'static,
    ) -> KeywordIndexCache {
        KeywordIndexCache {
            ttl,
            now: Box::new(now),
            entries: Mutex::new(HashMap::new()),
        }
    }

    async fn get_or_fetch<F, Fut>(
        &self,
        key: KeywordCacheKey,
        refresh: bool,
        fetch: F,
    ) -> Result<KeywordIndexLookup, ToolError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<RedactedKeywordWindow, ToolError>>,
    {
        let now = (self.now)();
        if !refresh && let Some(indexed) = self.fresh(&key, now) {
            return Ok(KeywordIndexLookup {
                indexed,
                cache_hit: true,
            });
        }

        let window = fetch().await?;
        let indexed = IndexedKeywordWindow::new(window);
        self.store(key, indexed.clone(), (self.now)());
        Ok(KeywordIndexLookup {
            indexed,
            cache_hit: false,
        })
    }

    fn fresh(&self, key: &KeywordCacheKey, now: Instant) -> Option<IndexedKeywordWindow> {
        self.entries
            .lock()
            .expect("keyword index cache")
            .get(key)
            .filter(|entry| now.saturating_duration_since(entry.minted_at) <= self.ttl)
            .map(|entry| entry.indexed.clone())
    }

    fn store(&self, key: KeywordCacheKey, indexed: IndexedKeywordWindow, minted_at: Instant) {
        self.entries
            .lock()
            .expect("keyword index cache")
            .insert(key, CachedKeywordWindow { minted_at, indexed });
    }
}

fn keyword_request_from_args(
    args: &serde_json::Value,
    match_pattern: String,
) -> Result<KeywordRequest, ToolError> {
    let pattern = string_arg(args, "pattern")?.to_string();
    if keyword_query_terms(&match_pattern).is_empty() {
        return Err(ToolError::InvalidArgs(
            "keyword log_grep pattern must contain at least one term".to_string(),
        ));
    }
    let level = optional_string_arg(args, "level")?
        .filter(|level| !level.is_empty())
        .map(ToOwned::to_owned);
    let limit = optional_usize_arg(args, "limit")?.unwrap_or(100);
    let refresh = optional_bool_arg(args, "refresh")?.unwrap_or(false);
    let profile_key = profile_key_from_args(args).to_string();
    Ok(KeywordRequest {
        pattern,
        match_pattern,
        level,
        limit,
        refresh,
        profile_key,
    })
}

#[cfg(test)]
fn filter_keyword_window(
    window: &RedactedKeywordWindow,
    request: &KeywordRequest,
) -> Result<CleanOutput, ToolError> {
    let index = KeywordIndex::build(&window.lines);
    filter_keyword_indexed_window(window, &index, request)
}

fn filter_keyword_indexed_window(
    window: &RedactedKeywordWindow,
    index: &KeywordIndex,
    request: &KeywordRequest,
) -> Result<CleanOutput, ToolError> {
    let terms = keyword_query_terms(&request.match_pattern);
    let level_re = request
        .level
        .as_ref()
        .map(|level| regex::Regex::new(&format!(r"(?i)\b{}\b", regex::escape(level))))
        .transpose()
        .map_err(|err| {
            ToolError::internal(LensError::Internal {
                detail: err.to_string(),
            })
        })?;
    let matched_indices = index
        .matching_lines(&terms)
        .into_iter()
        .filter(|index| {
            level_re
                .as_ref()
                .is_none_or(|level_re| level_re.is_match(&window.lines[*index]))
        })
        .collect::<Vec<_>>();
    let matched_lines = matched_indices.len();
    let returned_indices = matched_indices
        .iter()
        .copied()
        .take(request.limit)
        .collect::<Vec<_>>();
    let mut truncated_at = window.truncated_at.clone();
    if matched_lines > returned_indices.len() {
        push_truncation(&mut truncated_at, TruncatedAt::Rows);
    }
    let lines = returned_indices
        .iter()
        .map(|index| window.lines[*index].clone())
        .collect::<Vec<_>>();
    let text = render_keyword_output(
        window.metadata.as_ref(),
        request,
        &lines,
        matched_lines,
        &truncated_at,
    )?;
    Ok(CleanOutput::Text { text, truncated_at })
}

fn keyword_cache_key(ctx: &ToolCtx<'_>, request: &KeywordRequest) -> KeywordCacheKey {
    KeywordCacheKey::new(
        ctx.resources().session().audit_session_id().to_string(),
        request.profile_key.clone(),
    )
}

fn redacted_keyword_window_from_clean(
    clean: CleanOutput,
) -> Result<RedactedKeywordWindow, ToolError> {
    let CleanOutput::Text { text, truncated_at } = clean else {
        return Err(ToolError::internal(LensError::Internal {
            detail: "keyword log_grep expected text output".to_string(),
        }));
    };
    let (metadata, lines) = split_redacted_window_text(&text);
    Ok(RedactedKeywordWindow {
        lines,
        metadata,
        truncated_at,
    })
}

fn keyword_core_summary(window: &RedactedKeywordWindow, request: &KeywordRequest) -> ResultSummary {
    let text = render_keyword_core_window(window, request);
    CleanOutput::Text {
        text,
        truncated_at: window.truncated_at.clone(),
    }
    .summary()
}

fn render_keyword_core_window(window: &RedactedKeywordWindow, request: &KeywordRequest) -> String {
    let metadata = window
        .metadata
        .as_ref()
        .map(|metadata| keyword_core_metadata(metadata, request).to_string());
    let lines = window.lines.join("\n");
    match (metadata, lines.is_empty()) {
        (Some(metadata), false) => format!("{metadata}\n{lines}"),
        (Some(metadata), true) => metadata,
        (None, _) => lines,
    }
}

fn keyword_core_metadata(
    metadata: &serde_json::Value,
    request: &KeywordRequest,
) -> serde_json::Value {
    let serde_json::Value::Object(existing) = metadata else {
        return metadata.clone();
    };
    let mut metadata = existing.clone();
    metadata.insert(
        "pattern".to_string(),
        serde_json::Value::String(request.pattern.clone()),
    );
    metadata.insert(
        "level".to_string(),
        request
            .level
            .clone()
            .map(serde_json::Value::String)
            .unwrap_or(serde_json::Value::Null),
    );
    metadata.insert(
        "requested_limit".to_string(),
        serde_json::json!(request.limit),
    );
    serde_json::Value::Object(metadata)
}

fn split_redacted_window_text(text: &str) -> (Option<serde_json::Value>, Vec<String>) {
    let mut lines = text.lines();
    let Some(first) = lines.next() else {
        return (None, Vec::new());
    };
    if let Ok(metadata) = serde_json::from_str::<serde_json::Value>(first)
        && is_log_grep_metadata_header(&metadata)
    {
        return (Some(metadata), lines.map(ToOwned::to_owned).collect());
    }
    let mut out = vec![first.to_string()];
    out.extend(lines.map(ToOwned::to_owned));
    (None, out)
}

fn is_log_grep_metadata_header(metadata: &serde_json::Value) -> bool {
    let Some(source_kind) = metadata
        .get("source_kind")
        .and_then(serde_json::Value::as_str)
    else {
        return false;
    };
    metadata.as_object().is_some()
        && metadata
            .get("operation")
            .and_then(serde_json::Value::as_str)
            == Some("log_grep")
        && is_log_source_kind(source_kind)
        && (source_kind != "ssh_log" || has_string(metadata, "host"))
        && metadata
            .get("truncated_at")
            .and_then(serde_json::Value::as_array)
            .is_some()
        && has_string(metadata, "status")
        && has_string(metadata, "profile")
        && has_string(metadata, "path")
        && has_string(metadata, "pattern")
        && has_u64(metadata, "requested_limit")
        && has_u64(metadata, "tail_window_lines")
        && has_u64(metadata, "searched_lines")
        && has_u64(metadata, "matched_lines")
        && has_u64(metadata, "returned_lines")
        && has_u64(metadata, "searched_bytes")
}

fn is_log_source_kind(source_kind: &str) -> bool {
    matches!(source_kind, "ssh_log" | "local_log")
}

fn has_string(metadata: &serde_json::Value, key: &str) -> bool {
    metadata
        .get(key)
        .and_then(serde_json::Value::as_str)
        .is_some()
}

fn has_u64(metadata: &serde_json::Value, key: &str) -> bool {
    metadata
        .get(key)
        .and_then(serde_json::Value::as_u64)
        .is_some()
}

fn render_keyword_output(
    metadata: Option<&serde_json::Value>,
    request: &KeywordRequest,
    lines: &[String],
    matched_lines: usize,
    truncated_at: &[TruncatedAt],
) -> Result<String, ToolError> {
    let metadata = keyword_metadata(metadata, request, lines, matched_lines, truncated_at)?;
    let lines = lines.join("\n");
    match (metadata, lines.is_empty()) {
        (Some(metadata), false) => Ok(format!("{metadata}\n{lines}")),
        (Some(metadata), true) => Ok(metadata),
        (None, _) => Ok(lines),
    }
}

fn keyword_metadata(
    metadata: Option<&serde_json::Value>,
    request: &KeywordRequest,
    lines: &[String],
    matched_lines: usize,
    truncated_at: &[TruncatedAt],
) -> Result<Option<String>, ToolError> {
    if matched_lines != 0 && truncated_at.is_empty() {
        return Ok(None);
    }
    let returned_lines = lines.len();
    let status = keyword_status(matched_lines, truncated_at);
    let value = if let Some(serde_json::Value::Object(existing)) = metadata {
        let mut metadata = existing.clone();
        metadata.insert(
            "status".to_string(),
            serde_json::Value::String(status.to_string()),
        );
        metadata.insert(
            "pattern".to_string(),
            serde_json::Value::String(request.pattern.clone()),
        );
        metadata.insert(
            "level".to_string(),
            serde_json::to_value(&request.level).map_err(|err| {
                ToolError::internal(LensError::Internal {
                    detail: err.to_string(),
                })
            })?,
        );
        metadata.insert(
            "requested_limit".to_string(),
            serde_json::json!(request.limit),
        );
        metadata.insert(
            "matched_lines".to_string(),
            serde_json::json!(matched_lines),
        );
        metadata.insert(
            "returned_lines".to_string(),
            serde_json::json!(returned_lines),
        );
        metadata.insert(
            "truncated_at".to_string(),
            serde_json::to_value(truncated_at).map_err(|err| {
                ToolError::internal(LensError::Internal {
                    detail: err.to_string(),
                })
            })?,
        );
        serde_json::Value::Object(metadata)
    } else {
        serde_json::json!({
            "operation": "log_grep",
            "status": status,
            "pattern": request.pattern,
            "level": request.level,
            "requested_limit": request.limit,
            "matched_lines": matched_lines,
            "returned_lines": returned_lines,
            "truncated_at": truncated_at,
        })
    };
    serde_json::to_string(&value).map(Some).map_err(|err| {
        ToolError::internal(LensError::Internal {
            detail: err.to_string(),
        })
    })
}

fn keyword_status(matched_lines: usize, truncated_at: &[TruncatedAt]) -> &'static str {
    match (matched_lines, truncated_at.is_empty()) {
        (0, true) => "no_matches",
        (0, false) => "no_matches_truncated",
        (_, false) => "truncated",
        (_, true) => "matches",
    }
}

fn keyword_query_terms(pattern: &str) -> Vec<String> {
    pattern
        .split_whitespace()
        .filter_map(normalize_keyword_term)
        .collect()
}

fn indexed_line_terms(line: &str) -> BTreeSet<String> {
    let mut terms = BTreeSet::new();
    for chunk in line.split_whitespace() {
        if let Some(term) = normalize_keyword_term(chunk) {
            terms.insert(term);
        }
        collect_gaze_tokens(chunk, &mut terms);
        collect_keyword_segments(chunk, &mut terms);
    }
    terms
}

fn collect_gaze_tokens(chunk: &str, terms: &mut BTreeSet<String>) {
    let mut rest = chunk;
    while let Some(start) = rest.find('<') {
        let after_start = &rest[start..];
        let Some(end) = after_start.find('>') else {
            break;
        };
        let token = &after_start[..=end];
        if !token.chars().any(char::is_whitespace)
            && let Some(term) = normalize_keyword_term(token)
        {
            terms.insert(term);
        }
        rest = &after_start[end + 1..];
    }
}

fn collect_keyword_segments(chunk: &str, terms: &mut BTreeSet<String>) {
    let mut segment = String::new();
    let mut in_gaze_token = false;
    for ch in chunk.chars() {
        match ch {
            '<' => {
                push_segment(&mut segment, terms);
                in_gaze_token = true;
            }
            '>' if in_gaze_token => {
                in_gaze_token = false;
            }
            _ if in_gaze_token => {}
            _ if ch.is_alphanumeric() || ch == '_' => segment.push(ch),
            _ => push_segment(&mut segment, terms),
        }
    }
    push_segment(&mut segment, terms);
}

fn push_segment(segment: &mut String, terms: &mut BTreeSet<String>) {
    if let Some(term) = normalize_keyword_term(segment) {
        terms.insert(term);
    }
    segment.clear();
}

fn normalize_keyword_term(term: &str) -> Option<String> {
    let term = term.trim_matches(|ch: char| {
        ch.is_ascii_punctuation() && !matches!(ch, '<' | '>' | '_' | '-' | '=' | ':' | '@' | '.')
    });
    (!term.is_empty()).then(|| term.to_lowercase())
}

fn intersect_sorted(left: &[usize], right: &[usize]) -> Vec<usize> {
    let mut out = Vec::new();
    let mut left_index = 0;
    let mut right_index = 0;
    while left_index < left.len() && right_index < right.len() {
        match left[left_index].cmp(&right[right_index]) {
            std::cmp::Ordering::Less => left_index += 1,
            std::cmp::Ordering::Greater => right_index += 1,
            std::cmp::Ordering::Equal => {
                out.push(left[left_index]);
                left_index += 1;
                right_index += 1;
            }
        }
    }
    out
}

fn push_truncation(truncated_at: &mut Vec<TruncatedAt>, reason: TruncatedAt) {
    if !truncated_at.contains(&reason) {
        truncated_at.push(reason);
    }
}

fn string_arg<'a>(args: &'a serde_json::Value, key: &str) -> Result<&'a str, ToolError> {
    args.get(key)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| ToolError::InvalidArgs(format!("log_grep `{key}` must be a string")))
}

fn optional_string_arg<'a>(
    args: &'a serde_json::Value,
    key: &str,
) -> Result<Option<&'a str>, ToolError> {
    match args.get(key) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(value)) => Ok(Some(value)),
        Some(_) => Err(ToolError::InvalidArgs(format!(
            "log_grep `{key}` must be a string"
        ))),
    }
}

fn optional_usize_arg(args: &serde_json::Value, key: &str) -> Result<Option<usize>, ToolError> {
    match args.get(key) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::Number(value)) => value
            .as_u64()
            .and_then(|value| usize::try_from(value).ok())
            .map(Some)
            .ok_or_else(|| ToolError::InvalidArgs(format!("log_grep `{key}` must be a usize"))),
        Some(_) => Err(ToolError::InvalidArgs(format!(
            "log_grep `{key}` must be a usize"
        ))),
    }
}

fn optional_bool_arg(args: &serde_json::Value, key: &str) -> Result<Option<bool>, ToolError> {
    match args.get(key) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::Bool(value)) => Ok(Some(*value)),
        Some(_) => Err(ToolError::InvalidArgs(format!(
            "log_grep `{key}` must be a bool"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use gaze_mcp_core::manifest::{
        BeginCallContext, CallHandle, FailureReason, ManifestError, ManifestStore, SnapshotRef,
    };
    use gaze_mcp_core::{
        AuthError, AuthHook, DispatchError, PiiEnvelope, Principal, SessionIdPolicy, ToolError,
        ToolRegistry,
    };
    use serde_json::json;
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };
    use std::time::{Duration, Instant};

    use super::*;

    #[test]
    fn log_grep_mode_defaults_to_regex() {
        assert_eq!(
            log_grep_mode(&json!({"pattern": "ERROR"})).expect("mode"),
            LogGrepMode::Regex
        );
    }

    #[test]
    fn log_grep_mode_accepts_explicit_regex() {
        assert_eq!(
            log_grep_mode(&json!({"pattern": "ERROR", "mode": "regex"})).expect("mode"),
            LogGrepMode::Regex
        );
    }

    #[test]
    fn log_grep_mode_rejects_unknown_modes() {
        let err = log_grep_mode(&json!({"pattern": "ERROR", "mode": "substring"}))
            .expect_err("unknown mode");

        assert!(matches!(
            err,
            ToolError::InvalidArgs(message)
                if message.contains("invalid log_grep mode")
                    && message.contains("regex")
                    && message.contains("keyword")
        ));
    }

    #[tokio::test]
    async fn keyword_invoke_missing_scoped_pattern_returns_invalid_args() {
        let temp = tempfile::tempdir().expect("tempdir");
        let manifest_path = temp.path().join("manifest.sqlite");
        let snapshot_dir = temp.path().join("snapshots");
        let mut policy = gaze::Policy::default();
        policy.session.scope = gaze::SessionScope::Conversation;
        policy.rulepacks.bundled = vec!["core".to_string()];
        let lens_session =
            Session::new_with_pipeline(&policy, test_pipeline(), &manifest_path, &snapshot_dir)
                .expect("lens session");
        let tool = LogGrepTool::new(Arc::new(lens_session));
        let mut registry = ToolRegistry::new();
        registry.register(tool).expect("register log_grep");
        let auth = AllowAgentAuth;
        let manifest = NoopManifest;
        let pipeline = test_pipeline();
        let gaze_session =
            gaze::Session::new(gaze::Scope::Conversation(ulid::Ulid::new().to_string()))
                .expect("gaze session");
        let session_id_policy = SessionIdPolicy::default_strict();
        let envelope = PiiEnvelope::new(
            &registry,
            &auth,
            &manifest,
            &pipeline,
            &gaze_session,
            &[],
            &session_id_policy,
        );

        let err = RAW_LOG_GREP_PATTERN
            .scope(
                None,
                envelope.dispatch(
                    &Principal::new("test-agent"),
                    "log_grep",
                    json!({
                        "profile": "default",
                        "mode": "keyword"
                    }),
                    None,
                ),
            )
            .await
            .expect_err("missing pattern");

        assert!(matches!(
            err,
            DispatchError::ToolError(ToolError::InvalidArgs(message))
                if message == "log_grep `pattern` is required"
        ));
    }

    #[test]
    fn keyword_search_ands_terms_case_insensitively_in_original_order() {
        let window = redacted_window([
            "INFO release_id=43301 booted",
            "ERROR release_id=43301 first failure",
            "WARN release_id=43302 ignored",
            "error release_id=43301 second failure",
        ]);

        let clean = filter_keyword_window(&window, &keyword_request("ERROR 43301", 10))
            .expect("keyword search");

        assert_eq!(
            clean_text(clean),
            "ERROR release_id=43301 first failure\nerror release_id=43301 second failure"
        );
    }

    #[test]
    fn keyword_search_matches_gaze_tokens_literally() {
        let window = redacted_window([
            "ERROR actor=<EMAIL:Addr_1> failed",
            "ERROR actor=<EMAIL:Addr_2> failed",
        ]);

        let token_match = filter_keyword_window(&window, &keyword_request("<EMAIL:Addr_1>", 10))
            .expect("token search");
        assert_eq!(clean_text(token_match), "ERROR actor=<EMAIL:Addr_1> failed");

        let raw_match = filter_keyword_window(
            &window,
            &keyword_request_with_match_pattern("<EMAIL:Addr_1>", "alice@example.com", 10),
        )
        .expect("raw search");
        let raw_text = clean_text(raw_match);
        assert!(raw_text.contains(r#""status":"no_matches""#), "{raw_text}");
        assert!(
            raw_text.contains(r#""pattern":"<EMAIL:Addr_1>""#),
            "{raw_text}"
        );
        assert!(!raw_text.contains("alice@example.com"), "{raw_text}");
        assert!(!raw_text.contains("actor=<EMAIL:Addr_1>"), "{raw_text}");
    }

    #[test]
    fn keyword_request_normalizes_empty_level_to_absent() {
        let request = keyword_request_from_args(
            &json!({
                "profile": "prod-logs",
                "pattern": "43301",
                "level": "",
                "mode": "keyword"
            }),
            "43301".to_string(),
        )
        .expect("keyword request");

        assert_eq!(request.level, None);

        let window = redacted_window([
            "INFO release_id=43301 booted",
            "ERROR release_id=43301 failed",
        ]);
        let clean = filter_keyword_window(&window, &request).expect("keyword search");

        assert_eq!(
            clean_text(clean),
            "INFO release_id=43301 booted\nERROR release_id=43301 failed"
        );
    }

    #[test]
    fn keyword_search_honors_limit_with_rows_truncation() {
        let window = redacted_window([
            "ERROR release_id=43301 first",
            "ERROR release_id=43301 second",
            "ERROR release_id=43301 third",
        ]);

        let clean =
            filter_keyword_window(&window, &keyword_request("ERROR 43301", 2)).expect("search");

        let text = clean_text(clean.clone());
        let mut lines = text.lines();
        let metadata: serde_json::Value =
            serde_json::from_str(lines.next().expect("metadata")).expect("metadata json");
        assert_eq!(metadata["operation"], "log_grep");
        assert_eq!(metadata["status"], "truncated");
        assert_eq!(metadata["pattern"], "ERROR 43301");
        assert_eq!(metadata["level"], serde_json::Value::Null);
        assert_eq!(metadata["requested_limit"], 2);
        assert_eq!(metadata["matched_lines"], 3);
        assert_eq!(metadata["returned_lines"], 2);
        assert_eq!(metadata["truncated_at"], serde_json::json!(["Rows"]));
        assert_eq!(lines.next(), Some("ERROR release_id=43301 first"));
        assert_eq!(lines.next(), Some("ERROR release_id=43301 second"));
        assert_eq!(lines.next(), None);
        assert_eq!(
            clean_truncated_at(clean),
            vec![crate::session::TruncatedAt::Rows]
        );
    }

    #[test]
    fn keyword_search_rewrites_cached_metadata_for_current_request() {
        let mut window = redacted_window([
            "ERROR release_id=43301 first",
            "ERROR release_id=43301 second",
        ]);
        window.metadata = Some(serde_json::json!({
            "operation": "log_grep",
            "pattern": "stale",
            "level": "WARN",
            "requested_limit": 99,
            "matched_lines": 99,
            "returned_lines": 99,
            "truncated_at": []
        }));

        let clean =
            filter_keyword_window(&window, &keyword_request("ERROR 43301", 1)).expect("search");
        let text = clean_text(clean);
        let metadata: serde_json::Value =
            serde_json::from_str(text.lines().next().expect("metadata")).expect("metadata json");

        assert_eq!(metadata["pattern"], "ERROR 43301");
        assert_eq!(metadata["level"], serde_json::Value::Null);
        assert_eq!(metadata["requested_limit"], 1);
        assert_eq!(metadata["matched_lines"], 2);
        assert_eq!(metadata["returned_lines"], 1);
    }

    #[test]
    fn split_redacted_window_text_strips_log_grep_metadata_header() {
        let header = json!({
            "operation": "log_grep",
            "status": "matches",
            "profile": "prod-logs",
            "source_kind": "ssh_log",
            "host": "app.example",
            "path": "/var/log/app.log",
            "pattern": "43301",
            "level": null,
            "requested_limit": 100,
            "tail_window_lines": 10000,
            "searched_lines": 2,
            "matched_lines": 2,
            "returned_lines": 2,
            "searched_bytes": 42,
            "truncated_at": []
        });
        let text = format!("{header}\nERROR release_id=43301");

        let (metadata, lines) = split_redacted_window_text(&text);

        assert_eq!(metadata, Some(header));
        assert_eq!(lines, vec!["ERROR release_id=43301"]);
    }

    #[test]
    fn split_redacted_window_text_strips_local_log_metadata_header() {
        let header = json!({
            "operation": "log_grep",
            "status": "matches",
            "profile": "dev-log",
            "source_kind": "local_log",
            "path": "/tmp/app.log",
            "pattern": "43301",
            "level": null,
            "requested_limit": 100,
            "tail_window_lines": 10000,
            "searched_lines": 2,
            "matched_lines": 2,
            "returned_lines": 2,
            "searched_bytes": 42,
            "truncated_at": []
        });
        let text = format!("{header}\nERROR release_id=43301");

        let (metadata, lines) = split_redacted_window_text(&text);

        assert_eq!(metadata, Some(header));
        assert_eq!(lines, vec!["ERROR release_id=43301"]);
    }

    #[test]
    fn split_redacted_window_text_keeps_operation_json_log_line() {
        let first_line = r#"{"operation":"log_grep","message":"real log line"}"#;
        let text = format!("{first_line}\nERROR release_id=43301");

        let (metadata, lines) = split_redacted_window_text(&text);

        assert_eq!(metadata, None);
        assert_eq!(
            lines,
            vec![first_line.to_string(), "ERROR release_id=43301".to_string()]
        );
    }

    #[tokio::test]
    async fn keyword_index_cache_reuses_fresh_redacted_window() {
        let clock = test_clock();
        let cache = KeywordIndexCache::with_clock(Duration::from_secs(3), clock.now_fn());
        let key = KeywordCacheKey::new("session-a", "prod-logs");
        let fetches = Arc::new(AtomicUsize::new(0));

        let first = cache
            .get_or_fetch(key.clone(), false, {
                let fetches = Arc::clone(&fetches);
                || async move {
                    fetches.fetch_add(1, Ordering::SeqCst);
                    Ok(redacted_window(["ERROR release_id=43301 first"]))
                }
            })
            .await
            .expect("first fetch");
        assert!(!first.cache_hit);
        clock.advance(Duration::from_secs(1));
        let second = cache
            .get_or_fetch(key, false, {
                let fetches = Arc::clone(&fetches);
                || async move {
                    fetches.fetch_add(1, Ordering::SeqCst);
                    Ok(redacted_window(["ERROR release_id=43301 second"]))
                }
            })
            .await
            .expect("cached fetch");

        assert_eq!(fetches.load(Ordering::SeqCst), 1);
        assert!(second.cache_hit);
        assert_eq!(second.indexed, first.indexed);
    }

    #[tokio::test]
    async fn keyword_index_cache_refresh_busts_fresh_redacted_window() {
        let clock = test_clock();
        let cache = KeywordIndexCache::with_clock(Duration::from_secs(3), clock.now_fn());
        let key = KeywordCacheKey::new("session-a", "prod-logs");
        let fetches = Arc::new(AtomicUsize::new(0));

        cache
            .get_or_fetch(key.clone(), false, {
                let fetches = Arc::clone(&fetches);
                || async move {
                    fetches.fetch_add(1, Ordering::SeqCst);
                    Ok(redacted_window(["ERROR release_id=43301 first"]))
                }
            })
            .await
            .expect("first fetch");
        let refreshed = cache
            .get_or_fetch(key, true, {
                let fetches = Arc::clone(&fetches);
                || async move {
                    fetches.fetch_add(1, Ordering::SeqCst);
                    Ok(redacted_window(["ERROR release_id=43301 refreshed"]))
                }
            })
            .await
            .expect("refresh fetch");

        assert_eq!(fetches.load(Ordering::SeqCst), 2);
        assert!(!refreshed.cache_hit);
        assert_eq!(
            refreshed.indexed.window.lines,
            vec!["ERROR release_id=43301 refreshed"]
        );
    }

    #[tokio::test]
    async fn keyword_index_cache_refetches_stale_redacted_window() {
        let clock = test_clock();
        let cache = KeywordIndexCache::with_clock(Duration::from_secs(3), clock.now_fn());
        let key = KeywordCacheKey::new("session-a", "prod-logs");
        let fetches = Arc::new(AtomicUsize::new(0));

        cache
            .get_or_fetch(key.clone(), false, {
                let fetches = Arc::clone(&fetches);
                || async move {
                    fetches.fetch_add(1, Ordering::SeqCst);
                    Ok(redacted_window(["ERROR release_id=43301 first"]))
                }
            })
            .await
            .expect("first fetch");
        clock.advance(Duration::from_secs(4));
        let stale = cache
            .get_or_fetch(key, false, {
                let fetches = Arc::clone(&fetches);
                || async move {
                    fetches.fetch_add(1, Ordering::SeqCst);
                    Ok(redacted_window(["ERROR release_id=43301 stale"]))
                }
            })
            .await
            .expect("stale fetch");

        assert_eq!(fetches.load(Ordering::SeqCst), 2);
        assert!(!stale.cache_hit);
        assert_eq!(
            stale.indexed.window.lines,
            vec!["ERROR release_id=43301 stale"]
        );
    }

    fn redacted_window(lines: impl IntoIterator<Item = &'static str>) -> RedactedKeywordWindow {
        RedactedKeywordWindow {
            lines: lines.into_iter().map(ToString::to_string).collect(),
            metadata: None,
            truncated_at: Vec::new(),
        }
    }

    fn keyword_request(pattern: &str, limit: usize) -> KeywordRequest {
        keyword_request_with_match_pattern(pattern, pattern, limit)
    }

    fn keyword_request_with_match_pattern(
        pattern: &str,
        match_pattern: &str,
        limit: usize,
    ) -> KeywordRequest {
        KeywordRequest {
            pattern: pattern.to_string(),
            match_pattern: match_pattern.to_string(),
            level: None,
            limit,
            refresh: false,
            profile_key: "test".to_string(),
        }
    }

    fn clean_text(clean: crate::session::CleanOutput) -> String {
        match clean {
            crate::session::CleanOutput::Text { text, .. } => text,
            other => panic!("expected text output, got {other:?}"),
        }
    }

    fn clean_truncated_at(clean: crate::session::CleanOutput) -> Vec<crate::session::TruncatedAt> {
        match clean {
            crate::session::CleanOutput::Text { truncated_at, .. } => truncated_at,
            other => panic!("expected text output, got {other:?}"),
        }
    }

    fn test_clock() -> TestClock {
        TestClock {
            now: Arc::new(Mutex::new(Instant::now())),
        }
    }

    fn test_pipeline() -> gaze::Pipeline {
        gaze::Pipeline::builder().build().expect("pipeline build")
    }

    struct AllowAgentAuth;

    #[async_trait]
    impl AuthHook for AllowAgentAuth {
        async fn authorize_agent(
            &self,
            _principal: &Principal,
            _tool_name: &str,
        ) -> Result<(), AuthError> {
            Ok(())
        }

        async fn authorize_operator(
            &self,
            _principal: &Principal,
            _tool_name: &str,
        ) -> Result<(), AuthError> {
            Err(AuthError::Denied("agent-only test hook".to_string()))
        }
    }

    struct NoopManifest;

    #[async_trait]
    impl ManifestStore for NoopManifest {
        async fn begin_call(&self, ctx: BeginCallContext<'_>) -> Result<CallHandle, ManifestError> {
            Ok(CallHandle::new(ctx.call_id))
        }

        async fn finish_call(
            &self,
            _handle: CallHandle,
            _snapshot: SnapshotRef,
        ) -> Result<(), ManifestError> {
            Ok(())
        }

        async fn fail_call(
            &self,
            _handle: CallHandle,
            _reason: FailureReason,
        ) -> Result<(), ManifestError> {
            Ok(())
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
