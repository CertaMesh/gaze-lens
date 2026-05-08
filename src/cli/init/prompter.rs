//! Prompt abstraction for the guided init flow.
//!
//! `DialoguerPrompter` drives interactive TTY sessions; `FakePrompter` lets
//! unit tests script answers without spawning a pty. By default `FakePrompter`
//! errors on any unscripted call so empty fakes prove non-interactive paths
//! never prompt; tests that need default-fallthrough behavior must opt in
//! via `.allow_defaults()`.
//!
//! `last_prompt` records the most recent prompt message issued via any of the
//! four prompter methods. `last_select_choices` records the most recent select
//! list. They are `#[doc(hidden)] pub` (NOT `#[cfg(test)]`-gated) because
//! integration tests under `tests/*.rs` link the lib without `cfg(test)` and
//! therefore cannot reach `cfg(test)`-only symbols. Same exposure recipe as
//! `InitArgs::default_for_test`.

use std::collections::VecDeque;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum PromptError {
    #[error("user aborted prompt")]
    Aborted,
    #[error("not a tty; rerun with --non-interactive or --print-only")]
    NotATty,
    #[error("scripted prompter exhausted at {kind}")]
    ScriptExhausted { kind: &'static str },
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("dialoguer error: {0}")]
    Dialoguer(String),
}

pub trait Prompter {
    fn input(&mut self, msg: &str, default: Option<&str>) -> Result<String, PromptError>;
    fn confirm(&mut self, msg: &str, default: bool) -> Result<bool, PromptError>;
    fn select(&mut self, msg: &str, choices: &[&str]) -> Result<usize, PromptError>;
    fn password(&mut self, msg: &str) -> Result<String, PromptError>;
}

pub struct DialoguerPrompter {
    theme: dialoguer::theme::ColorfulTheme,
}

impl DialoguerPrompter {
    pub fn new() -> Self {
        Self {
            theme: dialoguer::theme::ColorfulTheme::default(),
        }
    }
}

impl Default for DialoguerPrompter {
    fn default() -> Self {
        Self::new()
    }
}

impl Prompter for DialoguerPrompter {
    fn input(&mut self, msg: &str, default: Option<&str>) -> Result<String, PromptError> {
        let mut builder = dialoguer::Input::<String>::with_theme(&self.theme);
        builder = builder.with_prompt(msg);
        if let Some(d) = default {
            builder = builder.default(d.to_string()).show_default(true);
        }
        builder.interact_text().map_err(|err| match err {
            dialoguer::Error::IO(io) => PromptError::Io(io),
        })
    }

    fn confirm(&mut self, msg: &str, default: bool) -> Result<bool, PromptError> {
        dialoguer::Confirm::with_theme(&self.theme)
            .with_prompt(msg)
            .default(default)
            .interact()
            .map_err(|err| match err {
                dialoguer::Error::IO(io) => PromptError::Io(io),
            })
    }

    fn select(&mut self, msg: &str, choices: &[&str]) -> Result<usize, PromptError> {
        dialoguer::Select::with_theme(&self.theme)
            .with_prompt(msg)
            .items(choices)
            .default(0)
            .interact()
            .map_err(|err| match err {
                dialoguer::Error::IO(io) => PromptError::Io(io),
            })
    }

    fn password(&mut self, msg: &str) -> Result<String, PromptError> {
        dialoguer::Password::with_theme(&self.theme)
            .with_prompt(msg)
            .interact()
            .map_err(|err| match err {
                dialoguer::Error::IO(io) => PromptError::Io(io),
            })
    }
}

#[derive(Default)]
pub struct FakePrompter {
    texts: VecDeque<String>,
    confirms: VecDeque<bool>,
    selects: VecDeque<usize>,
    passwords: VecDeque<String>,
    allow_defaults: bool,
    /// `#[doc(hidden)] pub` (NOT `#[cfg(test)]`). Integration tests under
    /// `tests/*.rs` compile without `cfg(test)` and need direct field access
    /// to assert prompt-template substrings.
    #[doc(hidden)]
    pub last_prompt: Option<String>,
    /// `#[doc(hidden)] pub` for the same reason as `last_prompt`.
    #[doc(hidden)]
    pub last_select_choices: Option<Vec<String>>,
}

impl FakePrompter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn allow_defaults(mut self) -> Self {
        self.allow_defaults = true;
        self
    }

    pub fn with_text(mut self, s: impl Into<String>) -> Self {
        self.texts.push_back(s.into());
        self
    }

    pub fn with_confirm(mut self, b: bool) -> Self {
        self.confirms.push_back(b);
        self
    }

    pub fn with_select(mut self, i: usize) -> Self {
        self.selects.push_back(i);
        self
    }

    pub fn with_password(mut self, s: impl Into<String>) -> Self {
        self.passwords.push_back(s.into());
        self
    }

    pub fn is_strict_and_empty(&self) -> bool {
        !self.allow_defaults
            && self.texts.is_empty()
            && self.confirms.is_empty()
            && self.selects.is_empty()
            && self.passwords.is_empty()
    }

    fn record(&mut self, msg: &str) {
        self.last_prompt = Some(msg.to_string());
    }
}

impl Prompter for FakePrompter {
    fn input(&mut self, msg: &str, default: Option<&str>) -> Result<String, PromptError> {
        self.record(msg);
        if let Some(s) = self.texts.pop_front() {
            return Ok(s);
        }
        if self.allow_defaults
            && let Some(d) = default
        {
            return Ok(d.to_string());
        }
        Err(PromptError::ScriptExhausted { kind: "text" })
    }

    fn confirm(&mut self, msg: &str, default: bool) -> Result<bool, PromptError> {
        self.record(msg);
        if let Some(b) = self.confirms.pop_front() {
            return Ok(b);
        }
        if self.allow_defaults {
            return Ok(default);
        }
        Err(PromptError::ScriptExhausted { kind: "confirm" })
    }

    fn select(&mut self, msg: &str, choices: &[&str]) -> Result<usize, PromptError> {
        self.record(msg);
        self.last_select_choices = Some(choices.iter().map(|choice| choice.to_string()).collect());
        self.selects
            .pop_front()
            .ok_or(PromptError::ScriptExhausted { kind: "select" })
    }

    fn password(&mut self, msg: &str) -> Result<String, PromptError> {
        self.record(msg);
        self.passwords
            .pop_front()
            .ok_or(PromptError::ScriptExhausted { kind: "password" })
    }
}
