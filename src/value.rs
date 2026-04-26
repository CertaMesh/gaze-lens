use std::collections::BTreeMap;

use gaze::{CleanDocument, RawDocument};
use thiserror::Error;

use crate::errors::LensError;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum LensValue {
    Null,
    Bool(bool),
    I64(i64),
    U64(u64),
    F64(f64),
    Decimal {
        value: String,
        precision: u8,
        scale: u8,
    },
    String(String),
    Bytes {
        base64: String,
        len: usize,
    },
    DateTime(String),
    Uuid(String),
    Json(serde_json::Value),
}

#[derive(Debug, Error, Clone, PartialEq)]
pub enum LowerError {
    #[error("decode failure for {kind}: {detail}")]
    Decode { kind: &'static str, detail: String },
    #[error("unsupported source type: {0}")]
    Unsupported(String),
}

impl LensValue {
    /// Lower to Gaze's current redaction value surface.
    ///
    /// String-like values lower to `gaze::Value::String`, `I64` lowers to
    /// `gaze::Value::I64`, and non-string typed values pass through unchanged
    /// by returning `Ok(None)`. Decode failures are explicit errors so upstream
    /// row conversion can reject the row.
    pub fn lower_for_redaction(&self) -> Result<Option<gaze::Value>, LowerError> {
        match self {
            Self::Null
            | Self::Bool(_)
            | Self::U64(_)
            | Self::F64(_)
            | Self::DateTime(_)
            | Self::Uuid(_) => Ok(None),
            Self::I64(value) => Ok(Some(gaze::Value::I64(*value))),
            Self::Decimal { value, .. } => {
                validate_decimal(value)?;
                Ok(None)
            }
            Self::String(value) => Ok(Some(gaze::Value::String(value.clone()))),
            Self::Json(value) => match value {
                serde_json::Value::String(text) => Ok(Some(gaze::Value::String(text.clone()))),
                _ => Ok(None),
            },
            Self::Bytes { .. } => Ok(None),
        }
    }

    pub fn redact_with(
        &mut self,
        gaze_session: &gaze::Session,
        pipeline: &gaze::Pipeline,
    ) -> Result<(), LensError> {
        match self {
            Self::String(text) => {
                *text = redact_text(gaze_session, pipeline, text.clone())?;
                Ok(())
            }
            Self::Json(value) => redact_json_value(value, gaze_session, pipeline),
            _ => {
                self.lower_for_redaction()?;
                Ok(())
            }
        }
    }
}

fn redact_json_value(
    value: &mut serde_json::Value,
    gaze_session: &gaze::Session,
    pipeline: &gaze::Pipeline,
) -> Result<(), LensError> {
    match value {
        serde_json::Value::String(text) => {
            *text = redact_text(gaze_session, pipeline, text.clone())?;
        }
        serde_json::Value::Array(values) => {
            for value in values {
                redact_json_value(value, gaze_session, pipeline)?;
            }
        }
        serde_json::Value::Object(values) => {
            for value in values.values_mut() {
                redact_json_value(value, gaze_session, pipeline)?;
            }
        }
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {}
    }
    Ok(())
}

fn redact_text(
    gaze_session: &gaze::Session,
    pipeline: &gaze::Pipeline,
    text: String,
) -> Result<String, LensError> {
    let clean = pipeline
        .redact(gaze_session, RawDocument::Text(text))
        .map_err(|err| LensError::RedactionFailed {
            detail: err.to_string(),
        })?;
    match clean {
        CleanDocument::Text(text) => Ok(text),
        CleanDocument::Structured(_) => Err(LensError::RedactionFailed {
            detail: "text value produced structured output".to_string(),
        }),
    }
}

fn validate_decimal(value: &str) -> Result<(), LowerError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(LowerError::Decode {
            kind: "decimal",
            detail: "empty decimal string".to_string(),
        });
    }

    let (mantissa, exponent) = value
        .split_once(['e', 'E'])
        .map_or((value, None), |(mantissa, exponent)| {
            (mantissa, Some(exponent))
        });
    validate_decimal_mantissa(mantissa)?;
    if let Some(exponent) = exponent {
        validate_decimal_exponent(exponent)?;
    }
    Ok(())
}

fn validate_decimal_mantissa(value: &str) -> Result<(), LowerError> {
    let digits = value.strip_prefix(['+', '-']).unwrap_or(value);
    let mut seen_digit = false;
    let mut seen_dot = false;
    for ch in digits.chars() {
        if ch.is_ascii_digit() {
            seen_digit = true;
            continue;
        }
        if ch == '.' && !seen_dot {
            seen_dot = true;
            continue;
        }
        return Err(LowerError::Decode {
            kind: "decimal",
            detail: format!("invalid character '{ch}'"),
        });
    }
    if seen_digit {
        Ok(())
    } else {
        Err(LowerError::Decode {
            kind: "decimal",
            detail: "missing digits".to_string(),
        })
    }
}

fn validate_decimal_exponent(value: &str) -> Result<(), LowerError> {
    let digits = value.strip_prefix(['+', '-']).unwrap_or(value);
    if !digits.is_empty() && digits.chars().all(|ch| ch.is_ascii_digit()) {
        Ok(())
    } else {
        Err(LowerError::Decode {
            kind: "decimal",
            detail: "invalid exponent".to_string(),
        })
    }
}

pub type LensRow = BTreeMap<String, LensValue>;

#[cfg(test)]
mod tests {
    use super::*;

    fn variants() -> Vec<LensValue> {
        vec![
            LensValue::Null,
            LensValue::Bool(true),
            LensValue::I64(-42),
            LensValue::U64(42),
            LensValue::F64(42.5),
            LensValue::Decimal {
                value: "123.45".to_string(),
                precision: 5,
                scale: 2,
            },
            LensValue::String("alice@example.com".to_string()),
            LensValue::Bytes {
                base64: "AQID".to_string(),
                len: 3,
            },
            LensValue::DateTime("2026-04-26T20:00:00Z".to_string()),
            LensValue::Uuid("018f3ec3-7b3a-7b24-a71d-5d34ec55acfd".to_string()),
            LensValue::Json(serde_json::json!({"email": "alice@example.com"})),
            LensValue::Json(serde_json::json!("alice@example.com")),
        ]
    }

    #[test]
    fn all_variants_round_trip_through_serde() {
        for value in variants() {
            let encoded = serde_json::to_string(&value).expect("encode");
            let decoded: LensValue = serde_json::from_str(&encoded).expect("decode");
            assert_eq!(decoded, value);
        }
    }

    #[test]
    fn string_values_lower_to_gaze_strings() {
        let value = LensValue::String("alice@example.com".to_string());
        let lowered = value.lower_for_redaction().expect("lower").expect("value");
        match lowered {
            gaze::Value::String(text) => assert_eq!(text, "alice@example.com"),
            gaze::Value::I64(_) => panic!("expected string"),
        }
    }

    #[test]
    fn string_json_lowers_to_gaze_string() {
        let value = LensValue::Json(serde_json::json!("alice@example.com"));
        let lowered = value.lower_for_redaction().expect("lower").expect("value");
        match lowered {
            gaze::Value::String(text) => assert_eq!(text, "alice@example.com"),
            gaze::Value::I64(_) => panic!("expected string"),
        }
    }

    #[test]
    fn scalar_non_string_values_pass_through() {
        let values = [
            LensValue::Null,
            LensValue::Bool(false),
            LensValue::U64(10),
            LensValue::F64(10.5),
            LensValue::Decimal {
                value: "10.5".to_string(),
                precision: 3,
                scale: 1,
            },
            LensValue::DateTime("2026-04-26T20:00:00Z".to_string()),
            LensValue::Uuid("018f3ec3-7b3a-7b24-a71d-5d34ec55acfd".to_string()),
            LensValue::Json(serde_json::json!({"email": "alice@example.com"})),
        ];

        for value in values {
            assert!(value.lower_for_redaction().expect("lower").is_none());
        }
    }

    #[test]
    fn i64_lowers_to_gaze_i64() {
        let lowered = LensValue::I64(-42)
            .lower_for_redaction()
            .expect("lower")
            .expect("value");
        match lowered {
            gaze::Value::I64(value) => assert_eq!(value, -42),
            gaze::Value::String(_) => panic!("expected i64"),
        }
    }

    #[test]
    fn corrupt_decimal_is_rejected() {
        let err = LensValue::Decimal {
            value: "not-a-number".to_string(),
            precision: 5,
            scale: 2,
        }
        .lower_for_redaction()
        .expect_err("corrupt decimal must fail");
        assert!(matches!(
            err,
            LowerError::Decode {
                kind: "decimal",
                ..
            }
        ));
    }

    #[test]
    fn bytes_pass_through_redaction_lowering() {
        let lowered = LensValue::Bytes {
            base64: "AQID".to_string(),
            len: 3,
        }
        .lower_for_redaction()
        .expect("lower");
        assert!(lowered.is_none());
    }
}
