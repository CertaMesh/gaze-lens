//! Lossless wire values. No Gaze conversion, source decoding or SQL binding.
use crate::{Error, Result, bounds};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::value::RawValue;

/// Validated JSON source bytes, preserving number lexemes and dynamic keys.
/// No Debug implementation: raw source content must not enter diagnostics.
pub struct ExactJson(Box<RawValue>);
impl ExactJson {
    pub fn parse(bytes: &str) -> Result<Self> {
        bounds::source_json(bytes.as_bytes())?;
        Ok(Self(
            RawValue::from_string(bytes.to_owned()).map_err(|_| Error::InvalidRequest)?,
        ))
    }
    pub fn as_str(&self) -> &str {
        self.0.get()
    }
}
impl Serialize for ExactJson {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        self.0.serialize(s)
    }
}
impl<'de> Deserialize<'de> for ExactJson {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let raw = Box::<RawValue>::deserialize(d)?;
        bounds::source_json(raw.get().as_bytes()).map_err(serde::de::Error::custom)?;
        Ok(Self(raw))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Semantic {
    Date,
    Time,
    Naive,
    Zoned,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Value {
    Null,
    Bool {
        value: bool,
    },
    I64 {
        value: String,
    },
    U64 {
        value: String,
    },
    F64 {
        value: String,
    },
    Decimal {
        value: String,
        precision: u8,
        scale: u8,
    },
    String {
        value: String,
    },
    Bytes {
        base64: String,
        len: usize,
    },
    Datetime {
        semantic: Semantic,
        value: String,
    },
    Uuid {
        value: String,
    },
    Json {
        value: ExactJson,
    },
}
#[derive(Deserialize)]
struct Tag {
    kind: String,
}
impl<'de> Deserialize<'de> for Value {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let raw = Box::<RawValue>::deserialize(d)?;
        decode(raw.get()).map_err(serde::de::Error::custom)
    }
}
fn decode(raw: &str) -> Result<Value> {
    bounds::require(raw.starts_with('{'))?;
    bounds::json(raw.as_bytes(), bounds::RESULT_BYTES)?;
    let tag: Tag = serde_json::from_str(raw).map_err(|_| Error::InvalidRequest)?;
    macro_rules! fields {
        ($($f:ident : $t:ty),* $(,)?) => {{
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Fields { #[serde(rename="kind")] _kind: String, $($f: $t),* }
            serde_json::from_str::<Fields>(raw).map_err(|_| Error::InvalidRequest)?
        }};
    }
    let v = match tag.kind.as_str() {
        "null" => {
            fields!();
            Value::Null
        }
        "bool" => Value::Bool {
            value: fields!(value: bool).value,
        },
        "i64" => Value::I64 {
            value: fields!(value: String).value,
        },
        "u64" => Value::U64 {
            value: fields!(value: String).value,
        },
        "f64" => Value::F64 {
            value: fields!(value: String).value,
        },
        "string" => Value::String {
            value: fields!(value: String).value,
        },
        "uuid" => Value::Uuid {
            value: fields!(value: String).value,
        },
        "json" => Value::Json {
            value: fields!(value: ExactJson).value,
        },
        "decimal" => {
            let f = fields!(value: String, precision: u8, scale: u8);
            Value::Decimal {
                value: f.value,
                precision: f.precision,
                scale: f.scale,
            }
        }
        "bytes" => {
            let f = fields!(base64: String, len: usize);
            Value::Bytes {
                base64: f.base64,
                len: f.len,
            }
        }
        "datetime" => {
            let f = fields!(semantic: Semantic, value: String);
            Value::Datetime {
                semantic: f.semantic,
                value: f.value,
            }
        }
        _ => return Err(Error::InvalidRequest),
    };
    v.validate()?;
    Ok(v)
}
impl Value {
    pub fn validate(&self) -> Result<()> {
        use bounds::require;
        match self {
            Self::Null | Self::Bool { .. } => Ok(()),
            Self::I64 { value } => {
                require(value.parse::<i64>().is_ok_and(|n| n.to_string() == *value))
            }
            Self::U64 { value } => {
                require(value.parse::<u64>().is_ok_and(|n| n.to_string() == *value))
            }
            Self::F64 { value } => {
                bounds::text(value)?;
                require(
                    bounds::number(value.as_bytes())
                        && value.parse::<f64>().is_ok_and(|n| {
                            n.is_finite() && decimal_equivalent(value, &n.to_string())
                        }),
                )
            }
            Self::String { value } => bounds::text(value),
            Self::Uuid { value } => require(
                value.len() == 36
                    && value.bytes().enumerate().all(|(i, b)| {
                        if matches!(i, 8 | 13 | 18 | 23) {
                            b == b'-'
                        } else {
                            b.is_ascii_digit() || (b'a'..=b'f').contains(&b)
                        }
                    }),
            ),
            Self::Decimal {
                value,
                precision,
                scale,
            } => {
                bounds::text(value)?;
                let (p, s) = decimal_metadata(value)?;
                require(usize::from(*precision) == p && usize::from(*scale) == s)
            }
            Self::Bytes { base64, len } => {
                bounds::cap(*len, bounds::SCALAR_BYTES)?;
                bounds::cap(base64.len(), bounds::SCALAR_BYTES.div_ceil(3) * 4)?;
                let bytes = STANDARD.decode(base64).map_err(|_| Error::InvalidRequest)?;
                require(bytes.len() == *len && STANDARD.encode(&bytes) == *base64)
            }
            Self::Datetime { semantic, value } => validate_datetime(*semantic, value),
            Self::Json { value } => bounds::source_json(value.as_str().as_bytes()),
        }
    }
    /// Normalize native decimal coefficient/scale without rounding or f64.
    /// Checks the full expansion before allocating; negative scales append zeros.
    pub fn decimal(coefficient: &str, native_scale: i64) -> Result<Self> {
        let digits = coefficient.strip_prefix('-').unwrap_or(coefficient);
        bounds::require(
            !digits.is_empty()
                && digits.bytes().all(|b| b.is_ascii_digit())
                && (digits.len() == 1 || !digits.starts_with('0')),
        )?;
        let scale = usize::try_from(native_scale.max(0)).map_err(|_| Error::CapExceeded)?;
        let zeros = if native_scale < 0 {
            usize::try_from(native_scale.unsigned_abs()).map_err(|_| Error::CapExceeded)?
        } else {
            0
        };
        let width = digits
            .len()
            .checked_add(zeros)
            .ok_or(Error::CapExceeded)?
            .max(scale);
        bounds::cap(width, 255)?;
        let mut out = String::new();
        out.try_reserve_exact(width + 3)
            .map_err(|_| Error::CapExceeded)?;
        if coefficient.starts_with('-') {
            out.push('-');
        }
        if scale == 0 {
            out.push_str(digits);
            if digits != "0" {
                out.extend(std::iter::repeat_n('0', zeros));
            }
        } else if digits.len() <= scale {
            out.push_str("0.");
            out.extend(std::iter::repeat_n('0', scale - digits.len()));
            out.push_str(digits);
        } else {
            let split = digits.len() - scale;
            out.push_str(&digits[..split]);
            out.push('.');
            out.push_str(&digits[split..]);
        }
        let (precision, scale) = decimal_metadata(&out)?;
        Ok(Self::Decimal {
            value: out,
            precision: precision as u8,
            scale: scale as u8,
        })
    }
}
fn decimal_metadata(value: &str) -> Result<(usize, usize)> {
    let s = value.strip_prefix('-').unwrap_or(value);
    let (int, frac) = s.split_once('.').map_or((s, None), |(a, b)| (a, Some(b)));
    bounds::require(
        !int.is_empty()
            && int.bytes().all(|b| b.is_ascii_digit())
            && (int.len() == 1 || !int.starts_with('0'))
            && frac.is_none_or(|f| !f.is_empty() && f.bytes().all(|b| b.is_ascii_digit())),
    )?;
    let scale = frac.map_or(0, str::len);
    let precision = (int.trim_start_matches('0').len() + scale).max(1);
    bounds::cap(precision, 255)?;
    Ok((precision, scale))
}
fn date(s: &str) -> bool {
    if s.len() != 10 || s.as_bytes()[4] != b'-' || s.as_bytes()[7] != b'-' {
        return false;
    }
    if !s
        .bytes()
        .enumerate()
        .all(|(i, b)| matches!(i, 4 | 7) || b.is_ascii_digit())
    {
        return false;
    }
    let Ok(y) = s[..4].parse::<u32>() else {
        return false;
    };
    let Ok(m) = s[5..7].parse::<u32>() else {
        return false;
    };
    let Ok(d) = s[8..].parse::<u32>() else {
        return false;
    };
    let max = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) => 29,
        2 => 28,
        _ => 0,
    };
    y > 0 && d > 0 && d <= max
}
fn time(s: &str) -> bool {
    let (main, frac) = s.split_once('.').map_or((s, None), |(a, b)| (a, Some(b)));
    if main.len() != 8
        || main.as_bytes()[2] != b':'
        || main.as_bytes()[5] != b':'
        || !main
            .bytes()
            .enumerate()
            .all(|(i, b)| matches!(i, 2 | 5) || b.is_ascii_digit())
    {
        return false;
    }
    main[..2].parse::<u8>().is_ok_and(|n| n < 24)
        && main[3..5].parse::<u8>().is_ok_and(|n| n < 60)
        && main[6..].parse::<u8>().is_ok_and(|n| n < 60)
        && frac.is_none_or(|f| !f.is_empty() && f.bytes().all(|b| b.is_ascii_digit()))
}
pub fn validate_datetime(semantic: Semantic, s: &str) -> Result<()> {
    bounds::text(s)?;
    bounds::require(s.is_ascii())?;
    let valid = match semantic {
        Semantic::Date => date(s),
        Semantic::Time => time(s),
        Semantic::Naive | Semantic::Zoned => {
            if let Some((d, t)) = s.split_once('T') {
                if semantic == Semantic::Naive {
                    date(d) && time(t)
                } else if let Some(t) = t.strip_suffix('Z') {
                    date(d) && time(t)
                } else if t.len() >= 6 {
                    let (t, z) = t.split_at(t.len() - 6);
                    date(d)
                        && time(t)
                        && matches!(z.as_bytes()[0], b'+' | b'-')
                        && z.as_bytes()[3] == b':'
                        && z[1..3].parse::<u8>().is_ok_and(|n| n < 24)
                        && z[4..].parse::<u8>().is_ok_and(|n| n < 60)
                        && z != "-00:00"
                } else {
                    false
                }
            } else {
                false
            }
        }
    };
    bounds::require(valid)
}

// Compare decimal spellings without exponent expansion. This rejects precision
// silently lost by parsing, while accepting equivalent exponent/trailing-zero forms.
fn decimal_equivalent(a: &str, b: &str) -> bool {
    fn parts(s: &str) -> Option<(bool, String, i64)> {
        let negative = s.starts_with('-');
        let s = s.strip_prefix('-').unwrap_or(s);
        let (mantissa, exp) = s
            .split_once(['e', 'E'])
            .map_or(Some((s, 0)), |(m, e)| e.parse::<i64>().ok().map(|e| (m, e)))?;
        let frac = mantissa.split_once('.').map_or(0, |(_, f)| f.len());
        let digits: String = mantissa
            .bytes()
            .filter(|b| *b != b'.')
            .map(char::from)
            .collect();
        let leading = digits.trim_start_matches('0');
        if leading.is_empty() {
            return Some((negative, String::new(), 0));
        }
        let trimmed = leading.trim_end_matches('0');
        let exponent = exp
            .checked_sub(i64::try_from(frac).ok()?)?
            .checked_add(i64::try_from(leading.len() - trimmed.len()).ok()?)?;
        Some((negative, trimmed.to_owned(), exponent))
    }
    parts(a).is_some_and(|a| parts(b).is_some_and(|b| a == b))
}
