use std::collections::HashMap;
use std::path::PathBuf;

use gaze::{
    Action, ClassRule as GazeClassRule, ColumnRule as GazeColumnRule, DefaultRule, PiiClass,
    Pipeline,
};
use gaze_recognizers::{NerDetector, NerOptions, RegexDetector};
use serde::Deserialize;
use thiserror::Error;

#[derive(Debug, Clone, Deserialize)]
pub struct PolicyFile {
    #[serde(default)]
    pub connection: HashMap<String, ConnectionConfig>,
    #[serde(default)]
    pub session: SessionConfig,
    #[serde(default)]
    pub ner: NerSection,
    pub policy: PolicySection,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ConnectionConfig {
    pub kind: String,
    #[serde(default)]
    pub ssh_host: Option<String>,
    #[serde(default)]
    pub local_port: Option<u16>,
    #[serde(default)]
    pub remote_host: Option<String>,
    #[serde(default)]
    pub remote_port: Option<u16>,
    #[serde(default)]
    pub database: Option<String>,
    #[serde(default)]
    pub user: Option<String>,
    #[serde(default)]
    pub password_env: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SessionConfig {
    #[serde(default = "default_session_scope")]
    pub scope: String,
    #[serde(default)]
    pub ttl_secs: Option<u64>,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            scope: default_session_scope(),
            ttl_secs: None,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct NerSection {
    #[serde(default)]
    pub model_dir: Option<PathBuf>,
    #[serde(default)]
    pub locale: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PolicySection {
    /// Action for detected spans without a more-specific column/class rule.
    /// Production-tier profiles should set this to `tokenize` or `redact`.
    #[serde(default)]
    pub default_action: Option<String>,
    pub database: DatabasePolicy,
    #[serde(default)]
    pub logs: Option<LogsPolicy>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct DatabasePolicy {
    #[serde(default, rename = "columns")]
    pub column_rules: Vec<ColumnRule>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ColumnRule {
    pub column: String,
    pub class: String,
    #[serde(default)]
    pub action: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct LogsPolicy {
    #[serde(default)]
    pub path: Option<PathBuf>,
    #[serde(default)]
    pub strip_patterns: Vec<String>,
    #[serde(default)]
    pub action: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ColumnAction {
    pub action: Action,
    pub class: PiiClass,
}

#[derive(Debug, Clone, Default)]
pub struct ColumnActionPolicy {
    actions: HashMap<String, ColumnAction>,
}

impl ColumnActionPolicy {
    pub fn from_policy_file(policy: &PolicyFile) -> Result<Self, PolicyError> {
        let mut actions = HashMap::new();
        for rule in &policy.policy.database.column_rules {
            actions.insert(
                rule.column.clone(),
                ColumnAction {
                    action: parse_action(
                        rule.action.as_deref().unwrap_or("tokenize"),
                        &rule.column,
                    )?,
                    class: parse_class(&rule.class, &rule.column)?,
                },
            );
        }
        Ok(Self { actions })
    }

    pub fn action_for(&self, column: &str) -> Option<&ColumnAction> {
        self.actions.get(column)
    }
}

#[derive(Debug, Error)]
pub enum PolicyError {
    #[error("failed to parse TOML: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("invalid action `{action}` for column `{column}`")]
    InvalidAction { column: String, action: String },
    #[error("unknown pii class `{class}` for column `{column}`")]
    UnknownPiiClass { column: String, class: String },
    #[error(
        "policy.session.scope = \"{scope}\" is not supported in v0.1; only \"conversation\" is accepted. See SPEC.md §session-lifecycle."
    )]
    UnsupportedSessionScope { scope: String },
    #[error("pipeline build failed: {0}")]
    Pipeline(#[from] gaze::Error),
    #[error("recognizer build failed: {0}")]
    Recognizer(String),
    #[error(
        "profile `{profile}` is marked `production = true` but its policy has no `[ner].model_dir`; production profiles require a configured NER model so person names cannot leak unredacted. Configure `[ner].model_dir` in the profile's policy file, or remove `production = true`."
    )]
    ProductionNerRequired { profile: String },
}

impl PolicyFile {
    pub fn from_toml(input: &str) -> Result<Self, PolicyError> {
        Ok(toml::from_str(input)?)
    }

    pub fn to_gaze_policy(&self) -> Result<gaze::Policy, PolicyError> {
        let scope = if self.session.scope.eq_ignore_ascii_case("conversation") {
            gaze::SessionScope::Conversation
        } else {
            return Err(PolicyError::UnsupportedSessionScope {
                scope: self.session.scope.clone(),
            });
        };
        let mut policy = gaze::Policy::default();
        policy.session.scope = scope;
        policy.session.ttl_secs = self.session.ttl_secs;
        policy.rulepacks.bundled = vec!["core".to_string()];
        Ok(policy)
    }
}

pub fn build_pipeline(policy: &PolicyFile) -> Result<Pipeline, PolicyError> {
    let mut builder = Pipeline::builder()
        .detector(RegexDetector::emails().map_err(|err| PolicyError::Recognizer(err.to_string()))?);

    if let Some(model_dir) = &policy.ner.model_dir {
        let detector = NerDetector::load_with_options(
            model_dir,
            NerOptions {
                locale: policy.ner.locale.clone(),
                threshold: gaze::DEFAULT_NER_THRESHOLD,
            },
        )
        .map_err(|err| {
            PolicyError::Pipeline(gaze::Error::Policy(gaze::PolicyError::NerLoad(
                err.to_string(),
            )))
        })?;
        builder = builder.detector(detector);
    }

    for rule in &policy.policy.database.column_rules {
        let action = parse_action(rule.action.as_deref().unwrap_or("tokenize"), &rule.column)?;
        let class = parse_class(&rule.class, &rule.column)?;
        builder = builder
            .rule(GazeColumnRule::new(&rule.column, action))
            .rule(GazeClassRule::new(class, action));
    }

    if let Some(logs) = &policy.policy.logs {
        for (index, pattern) in logs.strip_patterns.iter().enumerate() {
            builder = builder.detector(
                RegexDetector::with_source(
                    pattern,
                    PiiClass::custom("log_strip"),
                    &format!("log-strip-{index}"),
                )
                .map_err(|err| PolicyError::Recognizer(err.to_string()))?,
            );
        }
        if !logs.strip_patterns.is_empty() {
            let action = parse_log_strip_action(logs.action.as_deref())?;
            builder = builder.rule(GazeClassRule::new(PiiClass::custom("log_strip"), action));
        }
    }

    let default_action = parse_action(
        policy
            .policy
            .default_action
            .as_deref()
            .unwrap_or("preserve"),
        "policy.default_action",
    )?;

    Ok(builder.rule(DefaultRule::new(default_action)).build()?)
}

/// Enforce the production-profile NER mandate (#988).
///
/// A profile marked `production = true` MUST configure `[ner].model_dir` in its
/// policy. Without an NER model the pipeline only catches regex-detectable PII
/// (emails), so arbitrary person names — including names nested in JSON values —
/// would pass through unredacted. Since the gaze 0.11 bump makes NER fail-closed
/// (a model load/backend error aborts redaction rather than silently passing raw
/// text), it is now safe to *require* a model for production sources: a
/// misconfiguration fails closed at session build, never leaks at query time.
///
/// Non-production profiles are unaffected (the leak is opt-out-by-default; mark a
/// profile `production = true` to opt into the mandate).
pub fn enforce_production_ner(
    profile_name: &str,
    production: bool,
    policy: &PolicyFile,
) -> Result<(), PolicyError> {
    if production && policy.ner.model_dir.is_none() {
        return Err(PolicyError::ProductionNerRequired {
            profile: profile_name.to_string(),
        });
    }
    Ok(())
}

fn parse_action(raw: &str, column: &str) -> Result<Action, PolicyError> {
    match raw {
        "tokenize" => Ok(Action::Tokenize),
        "redact" => Ok(Action::Redact),
        "format_preserve" => Ok(Action::FormatPreserve),
        "generalize" => Ok(Action::Generalize),
        "preserve" => Ok(Action::Preserve),
        other => Err(PolicyError::InvalidAction {
            column: column.to_string(),
            action: other.to_string(),
        }),
    }
}

fn parse_log_strip_action(raw: Option<&str>) -> Result<Action, PolicyError> {
    let raw = raw.unwrap_or("redact");
    let action = parse_action(raw, "policy.logs.action")?;
    match action {
        Action::Redact | Action::Tokenize => Ok(action),
        _ => Err(PolicyError::InvalidAction {
            column: "policy.logs.action".to_string(),
            action: raw.to_string(),
        }),
    }
}

fn parse_class(raw: &str, column: &str) -> Result<PiiClass, PolicyError> {
    Ok(match raw {
        "email" => PiiClass::Email,
        "name" => PiiClass::Name,
        "location" => PiiClass::Location,
        "organization" => PiiClass::Organization,
        custom if !custom.trim().is_empty() => PiiClass::custom(custom),
        _ => {
            return Err(PolicyError::UnknownPiiClass {
                column: column.to_string(),
                class: raw.to_string(),
            });
        }
    })
}

fn default_session_scope() -> String {
    "conversation".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy_with_ner(model_dir: Option<&str>) -> PolicyFile {
        let mut policy = PolicyFile::from_toml("[policy.database]\n").expect("policy");
        policy.ner.model_dir = model_dir.map(std::path::PathBuf::from);
        policy
    }

    fn redact_policy_text(policy: &PolicyFile, text: &str) -> String {
        let pipeline = build_pipeline(policy).expect("pipeline");
        let session = gaze::Session::new(gaze::Scope::Conversation(ulid::Ulid::new().to_string()))
            .expect("gaze session");
        match pipeline
            .redact(&session, gaze::RawDocument::Text(text.to_string()))
            .expect("redact")
        {
            gaze::CleanDocument::Text(text) => text,
            other => panic!("expected text output, got {other:?}"),
        }
    }

    #[test]
    fn log_strip_patterns_remove_matching_text() {
        let policy = PolicyFile::from_toml(
            r#"
            [policy.database]

            [policy.logs]
            strip_patterns = ["Bob Marley"]
            "#,
        )
        .expect("policy");

        let output = redact_policy_text(&policy, "INFO customer=Bob Marley login=ok");

        assert!(
            !output.contains("Bob Marley"),
            "strip pattern survived redaction: {output}"
        );
    }

    #[test]
    fn production_profile_without_ner_is_rejected() {
        let policy = policy_with_ner(None);
        let err = enforce_production_ner("prod", true, &policy)
            .expect_err("production profile without ner.model_dir must fail closed");
        match err {
            PolicyError::ProductionNerRequired { profile } => assert_eq!(profile, "prod"),
            other => panic!("wrong error: {other}"),
        }
    }

    #[test]
    fn production_profile_with_ner_is_accepted() {
        let policy = policy_with_ner(Some("/models/ner"));
        enforce_production_ner("prod", true, &policy)
            .expect("production profile with a configured model passes the gate");
    }

    #[test]
    fn non_production_profile_without_ner_is_allowed() {
        // The leak is opt-out-by-default: only `production = true` profiles are
        // forced to configure NER. Non-production profiles keep the v1 behavior.
        let policy = policy_with_ner(None);
        enforce_production_ner("dev", false, &policy)
            .expect("non-production profile is not subject to the NER mandate");
    }
}
