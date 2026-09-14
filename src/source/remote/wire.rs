//! Private, bounded MCP transport. Remote values never enter the rmcp runtime.
use std::collections::HashSet;
use std::fmt;

use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};

use super::{Result, failure};

pub const VERSION: &str = "2025-11-25";
pub const FRAME_BYTES: usize = 1024 * 1024;
pub const RESULT_BYTES: usize = 128 * 1024;
pub const LINE_BYTES: usize = 8192;
pub const MAX_LINES: usize = 1000;
pub const SCAN_BYTES: usize = 1024 * 1024;

// Value's normal deserializer silently overwrites duplicate keys. Reject them at
// every depth before validating the exact protocol shape.
struct Unique(Value);
impl<'de> Deserialize<'de> for Unique {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Unique;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("unique JSON")
            }
            fn visit_bool<E: de::Error>(self, x: bool) -> std::result::Result<Unique, E> {
                Ok(Unique(x.into()))
            }
            fn visit_i64<E: de::Error>(self, x: i64) -> std::result::Result<Unique, E> {
                Ok(Unique(x.into()))
            }
            fn visit_u64<E: de::Error>(self, x: u64) -> std::result::Result<Unique, E> {
                Ok(Unique(x.into()))
            }
            fn visit_f64<E: de::Error>(self, x: f64) -> std::result::Result<Unique, E> {
                serde_json::Number::from_f64(x)
                    .map(|n| Unique(Value::Number(n)))
                    .ok_or_else(|| E::custom("invalid number"))
            }
            fn visit_str<E: de::Error>(self, x: &str) -> std::result::Result<Unique, E> {
                Ok(Unique(x.into()))
            }
            fn visit_string<E: de::Error>(self, x: String) -> std::result::Result<Unique, E> {
                Ok(Unique(x.into()))
            }
            fn visit_unit<E: de::Error>(self) -> std::result::Result<Unique, E> {
                Ok(Unique(Value::Null))
            }
            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut a: A,
            ) -> std::result::Result<Unique, A::Error> {
                let mut v = Vec::new();
                while let Some(x) = a.next_element::<Unique>()? {
                    v.push(x.0);
                }
                Ok(Unique(Value::Array(v)))
            }
            fn visit_map<A: MapAccess<'de>>(
                self,
                mut a: A,
            ) -> std::result::Result<Unique, A::Error> {
                let mut v = serde_json::Map::new();
                while let Some(k) = a.next_key::<String>()? {
                    if v.contains_key(&k) {
                        return Err(de::Error::custom("duplicate key"));
                    }
                    v.insert(k, a.next_value::<Unique>()?.0);
                }
                Ok(Unique(Value::Object(v)))
            }
        }
        d.deserialize_any(V)
    }
}

pub fn parse(bytes: &[u8]) -> Result<Value> {
    serde_json::from_slice::<Unique>(bytes)
        .map(|v| v.0)
        .map_err(|_| failure())
}

pub async fn read<R: AsyncBufRead + Unpin>(io: &mut R) -> Result<Value> {
    tokio::time::timeout(std::time::Duration::from_secs(10), read_frame(io))
        .await
        .map_err(|_| failure())?
}
async fn read_frame<R: AsyncBufRead + Unpin>(io: &mut R) -> Result<Value> {
    let mut bytes = Vec::new();
    loop {
        let buf = io.fill_buf().await.map_err(|_| failure())?;
        if buf.is_empty() {
            return Err(failure());
        }
        let n = buf
            .iter()
            .position(|b| *b == b'\n')
            .map_or(buf.len(), |i| i + 1);
        if bytes.len() + n > FRAME_BYTES {
            return Err(failure());
        }
        let done = buf[n - 1] == b'\n';
        bytes.extend_from_slice(&buf[..n]);
        io.consume(n);
        if done {
            return parse(&bytes);
        }
    }
}

pub async fn write<W: AsyncWrite + Unpin>(io: &mut W, value: &Value) -> Result<()> {
    let mut bytes = serde_json::to_vec(value).map_err(|_| failure())?;
    if bytes.len() >= FRAME_BYTES {
        return Err(failure());
    }
    bytes.push(b'\n');
    io.write_all(&bytes).await.map_err(|_| failure())?;
    io.flush().await.map_err(|_| failure())
}

pub async fn expect<R: AsyncBufRead + Unpin>(io: &mut R, value: Value) -> Result<()> {
    if read(io).await? != value {
        return Err(failure());
    }
    Ok(())
}

pub fn initialize() -> Value {
    json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":VERSION,"capabilities":{},"clientInfo":{"name":"gaze-lens","version":"1"}}})
}
pub fn initialized() -> Value {
    json!({"jsonrpc":"2.0","method":"notifications/initialized"})
}
pub fn ready() -> Value {
    json!({"jsonrpc":"2.0","id":1,"result":{"protocolVersion":VERSION,"capabilities":{"tools":{}},"serverInfo":{"name":"gaze-lens-log-service","version":"1"}}})
}
pub fn list() -> Value {
    json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}})
}
pub fn tools() -> Value {
    json!({"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"log_tail","description":"Read a bounded configured log resource","inputSchema":{"type":"object","properties":{"resource":{"type":"string"},"lines":{"type":"integer","minimum":1,"maximum":MAX_LINES}},"required":["resource","lines"],"additionalProperties":false}}]}})
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Call {
    pub jsonrpc: String,
    pub id: u64,
    pub method: String,
    pub params: Params,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Params {
    pub name: String,
    pub arguments: Arguments,
    #[serde(rename = "_meta")]
    pub meta: Authentication,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Authentication {
    #[serde(rename = "gaze-lens/token")]
    pub token: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Arguments {
    pub resource: String,
    pub lines: usize,
}

pub fn call(resource: &str, lines: usize, token: &str) -> Value {
    json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"log_tail","arguments":{"resource":resource,"lines":lines},"_meta":{"gaze-lens/token":token}}})
}
pub fn decode_call(value: Value) -> Result<Call> {
    let c: Call = serde_json::from_value(value).map_err(|_| failure())?;
    if c.jsonrpc != "2.0"
        || c.id != 3
        || c.method != "tools/call"
        || c.params.name != "log_tail"
        || !(1..=MAX_LINES).contains(&c.params.arguments.lines)
        || !valid_name(&c.params.arguments.resource)
        || !valid_token(&c.params.meta.token)
    {
        return Err(failure());
    }
    Ok(c)
}
pub fn valid_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
pub fn valid_token(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogBody {
    pub lines: Vec<String>,
    pub truncated: Vec<Truncation>,
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Truncation {
    Bytes,
    Lines,
    LineBytes,
}

/// The only conversion from a validated upstream body into a redaction input.
pub struct UpstreamText(LogBody);
impl UpstreamText {
    pub fn validate(body: LogBody, requested: usize) -> Result<Self> {
        let mut total = 0usize;
        let mut seen = HashSet::new();
        if body.lines.len() > requested
            || body.lines.len() > MAX_LINES
            || body.truncated.iter().any(|x| !seen.insert(*x))
        {
            return Err(failure());
        }
        for line in &body.lines {
            total = total.checked_add(line.len() + 1).ok_or_else(failure)?;
            if line.len() > LINE_BYTES || line.contains(['\n', '\r', '\0']) || total > RESULT_BYTES
            {
                return Err(failure());
            }
        }
        Ok(Self(body))
    }
    pub fn into_output(self) -> crate::source::SourceOutput {
        use crate::session::TruncatedAt;
        crate::source::SourceOutput::TextWithTruncation {
            text: self.0.lines.join("\n"),
            truncated_at: self
                .0
                .truncated
                .into_iter()
                .map(|t| match t {
                    Truncation::Bytes => TruncatedAt::Bytes,
                    Truncation::Lines => TruncatedAt::Rows,
                    Truncation::LineBytes => TruncatedAt::LineBytes,
                })
                .collect(),
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Response {
    jsonrpc: String,
    id: u64,
    result: CallResult,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct CallResult {
    content: Vec<Value>,
    structured_content: LogBody,
    is_error: bool,
}
pub fn decode_response(value: Value, requested: usize) -> Result<UpstreamText> {
    let r: Response = serde_json::from_value(value).map_err(|_| failure())?;
    if r.jsonrpc != "2.0" || r.id != 3 || r.result.is_error || !r.result.content.is_empty() {
        return Err(failure());
    }
    UpstreamText::validate(r.result.structured_content, requested)
}
pub fn response(body: LogBody) -> Value {
    json!({"jsonrpc":"2.0","id":3,"result":{"content":[],"structuredContent":body,"isError":false}})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn duplicate_keys_and_deep_or_invalid_json_reject() {
        for bytes in [
            br#"{"a":1,"a":2}"#.as_slice(),
            br#"{"outer":{"a":1,"a":2}}"#,
            br#"[ {"a":1,"a":2} ]"#,
            b"\xff",
            b"{} {}",
        ] {
            assert!(parse(bytes).is_err());
        }
        assert!(parse(format!("{}0{}", "[".repeat(200), "]".repeat(200)).as_bytes()).is_err());
        assert_eq!(VERSION, rmcp::model::ProtocolVersion::LATEST.to_string());
    }
    #[test]
    fn strict_control_and_result_shapes() {
        let base = response(LogBody {
            lines: vec!["sensitive@example.test".into()],
            truncated: vec![],
        });
        assert!(decode_response(base.clone(), 1).is_ok());
        for (pointer, value) in [
            ("/result/isError", json!(true)),
            ("/id", json!(9)),
            ("/result/content", json!([{"type":"text","text":"canary"}])),
        ] {
            let mut invalid = base.clone();
            *invalid.pointer_mut(pointer).unwrap() = value;
            assert!(decode_response(invalid, 1).is_err());
        }
        for key in ["_meta", "instructions", "annotations", "error"] {
            let mut invalid = base.clone();
            invalid
                .as_object_mut()
                .unwrap()
                .insert(key.into(), json!("canary"));
            assert!(decode_response(invalid, 1).is_err());
            let mut invalid = base.clone();
            invalid["result"]
                .as_object_mut()
                .unwrap()
                .insert(key.into(), json!("canary"));
            assert!(decode_response(invalid, 1).is_err());
        }
        let mut invalid = call("app", 1, &"a".repeat(64));
        invalid["params"]["arguments"]["path"] = json!("/secret");
        assert!(decode_call(invalid).is_err());
    }
    #[tokio::test]
    async fn bounded_frames_reject_unterminated_oversize() {
        let bytes = vec![b'x'; FRAME_BYTES + 1];
        let mut io = std::io::Cursor::new(bytes);
        assert!(read(&mut io).await.is_err());
    }
    #[test]
    fn line_shapes_reject_without_clipping() {
        for line in ["a\nb".into(), "a\0b".into(), "x".repeat(LINE_BYTES + 1)] {
            assert!(
                UpstreamText::validate(
                    LogBody {
                        lines: vec![line],
                        truncated: vec![]
                    },
                    1
                )
                .is_err()
            );
        }
        assert!(
            UpstreamText::validate(
                LogBody {
                    lines: vec![],
                    truncated: vec![Truncation::Bytes, Truncation::Bytes]
                },
                1
            )
            .is_err()
        );
    }
}
