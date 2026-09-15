use gaze_lens_protocol::{
    bounds,
    value::{ExactJson, Semantic, Value, validate_datetime},
    wire::*,
};
use serde_json::{Value as Json, json};
const ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
fn binding() -> Json {
    json!({"principal":"a".repeat(32),"principal_generation":"b".repeat(32),"resource":"c".repeat(32),"resource_generation":"d".repeat(32)})
}
fn frame(v: Json) -> Vec<u8> {
    let mut b = serde_json::to_vec(&v).unwrap();
    b.push(b'\n');
    b
}
fn call(op: &str, args: Json) -> Call {
    decode_call(&frame(
        json!({"version":VERSION,"id":ID,"operation":op,"binding":binding(),"args":args}),
    ))
    .unwrap()
}
fn prepare() -> Prepare {
    decode_prepare(&frame(json!({"version":VERSION,"privacy":"client_gaze","id":ID,"credential":"a".repeat(64),"resource":"db","operation":"readiness"}))).unwrap()
}
fn prepared() -> Prepared {
    decode_prepared(&frame(json!({"version":VERSION,"privacy":"client_gaze","id":ID,"operation":"readiness","binding":binding()}))).unwrap()
}
fn sequence() -> Sequence {
    let mut s = Sequence::default();
    s.prepare(prepare()).unwrap();
    s.prepared(prepared()).unwrap();
    s.call(call("readiness", json!({}))).unwrap();
    s
}
#[test]
fn zoned_offset_components_require_digits() {
    for s in [
        "2024-01-01T00:00:00++1:00",
        "2024-01-01T00:00:00+01:+1",
        "2024-01-01T00:00:00-+1:00",
    ] {
        assert!(
            validate_datetime(Semantic::Zoned, s).is_err(),
            "accepted invalid RFC3339: {s}"
        );
    }
}
#[test]
fn valid_multiline_native_json_can_be_framed_without_losing_lexemes() {
    let source = "{\n  \"9007199254740993\": [0.123456789012345678901,1.2300e+1000],\n  \"$serde_json::private::Number\": \"literal\\ntext\"\n}";
    let value = ExactJson::parse(source).unwrap();
    assert_eq!(
        value.as_str(),
        r#"{"9007199254740993":[0.123456789012345678901,1.2300e+1000],"$serde_json::private::Number":"literal\ntext"}"#
    );
    let escaped = "{\r\n\t\"a\\u0062\" : [ -0, 1e99999999999999999999999999999999999 ], \"s\" : \" a \\t \\u0020 \" }";
    let value_from_serde: ExactJson = serde_json::from_str(escaped).unwrap();
    assert_eq!(
        value_from_serde.as_str(),
        r#"{"a\u0062":[-0,1e99999999999999999999999999999999999],"s":" a \t \u0020 "}"#
    );
    let c = call("query", json!({"table":"t"}));
    let s = Success {
        id: ID.into(),
        operation: Operation::Query,
        binding: c.binding.clone(),
        result: ResultBody::QueryRows {
            columns: vec!["x".into()],
            rows: vec![vec![Value::Json { value }]],
            truncated: vec![],
        },
    };
    s.result.validate_for(&c.args).unwrap();
    let encoded =
        encode_success(&s, &c).expect("valid exact JSON must be representable on one NDJSON frame");
    assert_eq!(encoded.iter().filter(|b| **b == b'\n').count(), 1);
    let decoded = decode_success(&encoded, &c).unwrap();
    let ResultBody::QueryRows { rows, .. } = decoded.result else {
        panic!()
    };
    let Value::Json { value } = &rows[0][0] else {
        panic!()
    };
    for lexeme in [
        "9007199254740993",
        "0.123456789012345678901",
        "1.2300e+1000",
        "$serde_json::private::Number",
        "literal\\ntext",
    ] {
        assert!(value.as_str().contains(lexeme));
    }
}
#[test]
fn every_sequence_terminal_error_closes() {
    for bytes in [
        b"{}\n".as_slice(),
        b"[]\n",
        b"null\n",
        b"{bad}\n",
        b"{}\n{}\n",
    ] {
        let mut s = sequence();
        assert!(s.success(bytes).is_err());
        assert!(s.prepare(prepare()).is_err());
        let mut s = sequence();
        assert!(s.failure(bytes).is_err());
        assert!(s.prepare(prepare()).is_err());
    }
    let mut s = Sequence::default();
    s.prepare(prepare()).unwrap();
    let mut p = prepared();
    p.operation = Operation::Query;
    assert!(s.prepared(p).is_err());
    assert!(s.prepared(prepared()).is_err());
    let mut s = Sequence::default();
    s.prepare(prepare()).unwrap();
    s.prepared(prepared()).unwrap();
    assert!(s.call(call("list_tables", json!({}))).is_err());
    assert!(s.call(call("readiness", json!({}))).is_err());
}
#[test]
fn frame_admission_chunk_boundaries_and_poison() {
    let b = frame(json!({"x":"escaped\\nnot-newline"}));
    for split in 0..b.len() {
        let mut f = bounds::FrameBuffer::new(b.len()).unwrap();
        f.push(&b[..split]).unwrap();
        assert!(f.push(&b[split..]).unwrap());
        assert_eq!(f.finish().unwrap(), b);
    }
    for bad in [b"{}\n ".as_slice(), b"{}\n{}\n", b"123456789"] {
        let mut f = bounds::FrameBuffer::new(8).unwrap();
        assert!(f.push(bad).is_err());
        assert!(f.push(b"\n").is_err());
        assert!(f.finish().is_err());
    }
}
#[test]
fn scanner_denies_escaped_duplicates_and_invalid_unicode() {
    for s in [
        r#"{"😀":1,"\ud83d\ude00":2}"#,
        r#"{"a":1,"\u0061":2}"#,
        r#""\ud800""#,
        r#""\udc00""#,
        r#""\x00""#,
        r#"[01]"#,
        r#"[1e+]"#,
        r#"[true,]"#,
        r#"{"a":0,}"#,
    ] {
        assert!(bounds::source_json(s.as_bytes()).is_err(), "{s}");
    }
    assert!(bounds::source_json(&[b'"', 0xff, b'"']).is_err());
    let exact = format!("\"{}\"", "\\u0061".repeat(8192));
    assert!(ExactJson::parse(&exact).is_ok());
    let over = format!("\"{}\"", "\\u0061".repeat(8193));
    assert!(ExactJson::parse(&over).is_err());
}
#[test]
fn required_nullable_fields_and_optional_null_are_distinct() {
    for args in [
        json!({"table":"t","columns":null}),
        json!({"table":"t","where":null}),
        json!({"table":"t","order_by":null}),
        json!({"table":"t","where_combinator":null}),
    ] {
        assert!(decode_call(&frame(json!({"version":VERSION,"id":ID,"operation":"query","binding":binding(),"args":args}))).is_err());
    }
    for args in [
        json!({"pattern":"x","level":null,"limit":1}),
        json!(["x", null, 1]),
    ] {
        assert!(decode_call(&frame(json!({"version":VERSION,"id":ID,"operation":"log_regex","binding":binding(),"args":args}))).is_err());
    }
}
#[test]
fn exact_number_and_dynamic_key_semantics_survive_full_wire() {
    let source = r#"{"$serde_json::private::Number":{"$serde_json::private::RawValue":9007199254740993},"a\u0062":[-0,1e99999999999999999999999999999999999,0.000000000000000000000000000001]}"#;
    let c = call("query", json!({"table":"t"}));
    let s = Success {
        id: ID.into(),
        operation: Operation::Query,
        binding: c.binding.clone(),
        result: ResultBody::QueryRows {
            columns: vec!["x".into()],
            rows: vec![vec![Value::Json {
                value: ExactJson::parse(source).unwrap(),
            }]],
            truncated: vec![],
        },
    };
    let bytes = encode_success(&s, &c).unwrap();
    assert!(String::from_utf8(bytes.clone()).unwrap().contains(source));
    assert!(decode_success(&bytes, &c).is_ok());
}
#[test]
fn temporal_positive_and_negative_boundaries() {
    for (kind, s) in [
        (Semantic::Date, "2000-02-29"),
        (Semantic::Time, "23:59:59.999999"),
        (Semantic::Naive, "2024-01-01T00:00:00"),
        (Semantic::Zoned, "2024-01-01T00:00:00+00:00"),
        (Semantic::Zoned, "2024-02-29T12:34:56.123456+05:30"),
        (Semantic::Zoned, "2024-02-29T12:34:56Z"),
        (Semantic::Zoned, "2024-02-29T12:34:56-01:30"),
    ] {
        assert!(validate_datetime(kind, s).is_ok());
    }
    for (kind, s) in [
        (Semantic::Date, "1900-02-29"),
        (Semantic::Time, "24:00:00"),
        (Semantic::Naive, "2024-01-01T00:00:00Z"),
        (Semantic::Zoned, "2024-01-01T00:00:00-00:00"),
        (Semantic::Zoned, "2024-01-01T00:00:00+24:00"),
    ] {
        assert!(validate_datetime(kind, s).is_err());
    }
}
