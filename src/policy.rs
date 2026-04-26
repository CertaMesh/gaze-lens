use std::collections::HashMap;
use std::path::PathBuf;

use gaze::{
    Action, ClassRule as GazeClassRule, ColumnRule as GazeColumnRule, DefaultRule, PiiClass,
    Pipeline,
};
use gaze_recognizers::{NerDetector, NerOptions, RegexDetector};
use serde::Deserialize;
use thiserror::Error;

pub const SCHEMA_METADATA_SOURCE_CLASS: &str = "schema_metadata";

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
}

#[derive(Debug, Error)]
pub enum PolicyError {
    #[error("failed to parse TOML: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("invalid action `{action}` for column `{column}`")]
    InvalidAction { column: String, action: String },
    #[error("unknown pii class `{class}` for column `{column}`")]
    UnknownPiiClass { column: String, class: String },
    #[error("invalid session scope `{0}`")]
    InvalidSessionScope(String),
    #[error("pipeline build failed: {0}")]
    Pipeline(#[from] gaze::Error),
    #[error("recognizer build failed: {0}")]
    Recognizer(String),
}

impl PolicyFile {
    pub fn from_toml(input: &str) -> Result<Self, PolicyError> {
        Ok(toml::from_str(input)?)
    }

    pub fn to_gaze_policy(&self) -> Result<gaze::Policy, PolicyError> {
        let scope = match self.session.scope.as_str() {
            "ephemeral" => gaze::SessionScope::Ephemeral,
            "conversation" => gaze::SessionScope::Conversation,
            "persistent" => gaze::SessionScope::Persistent,
            other => return Err(PolicyError::InvalidSessionScope(other.to_string())),
        };
        Ok(gaze::Policy {
            session: gaze::SessionPolicy {
                scope,
                ttl_secs: self.session.ttl_secs,
            },
            detectors: Vec::new(),
            dictionaries: Vec::new(),
            rules: Vec::new(),
            ner: None,
            rulepacks: gaze::RulepackPolicy {
                bundled: vec!["core".to_string()],
                paths: Vec::new(),
            },
            locale: None,
        })
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
    }

    Ok(builder.rule(DefaultRule::new(Action::Preserve)).build()?)
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
