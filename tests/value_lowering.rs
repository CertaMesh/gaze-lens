use gaze_lens::value::{LensValue, LowerError};

fn all_variants() -> Vec<LensValue> {
    vec![
        LensValue::Null,
        LensValue::Bool(true),
        LensValue::I64(-1),
        LensValue::U64(1),
        LensValue::F64(1.5),
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
        LensValue::Json(serde_json::json!({"nested": "alice@example.com"})),
    ]
}

#[test]
fn round_trips_every_lens_value_variant() {
    for value in all_variants() {
        let encoded = serde_json::to_string(&value).expect("encode");
        let decoded: LensValue = serde_json::from_str(&encoded).expect("decode");
        assert_eq!(decoded, value);
    }
}

#[test]
fn lowers_strings_for_redaction() {
    let lowered = LensValue::String("alice@example.com".to_string())
        .lower_for_redaction()
        .expect("lower")
        .expect("lowered");

    match lowered {
        gaze::Value::String(text) => assert_eq!(text, "alice@example.com"),
        gaze::Value::I64(_) => panic!("expected gaze string"),
    }
}

#[test]
fn lowers_string_json_for_redaction() {
    let lowered = LensValue::Json(serde_json::json!("alice@example.com"))
        .lower_for_redaction()
        .expect("lower")
        .expect("lowered");

    match lowered {
        gaze::Value::String(text) => assert_eq!(text, "alice@example.com"),
        gaze::Value::I64(_) => panic!("expected gaze string"),
    }
}

#[test]
fn passes_through_non_string_values() {
    let values = [
        LensValue::Null,
        LensValue::Bool(true),
        LensValue::U64(1),
        LensValue::F64(1.5),
        LensValue::Decimal {
            value: "-123.45e2".to_string(),
            precision: 7,
            scale: 2,
        },
        LensValue::DateTime("2026-04-26T20:00:00Z".to_string()),
        LensValue::Uuid("018f3ec3-7b3a-7b24-a71d-5d34ec55acfd".to_string()),
    ];

    for value in values {
        assert!(value.lower_for_redaction().expect("lower").is_none());
    }
}

#[test]
fn lowers_i64_for_redaction() {
    let lowered = LensValue::I64(-10)
        .lower_for_redaction()
        .expect("lower")
        .expect("lowered");

    match lowered {
        gaze::Value::I64(value) => assert_eq!(value, -10),
        gaze::Value::String(_) => panic!("expected gaze i64"),
    }
}

#[test]
fn rejects_bytes_until_v1_policy_exists() {
    let err = LensValue::Bytes {
        base64: "AQID".to_string(),
        len: 3,
    }
    .lower_for_redaction()
    .expect_err("bytes should be unsupported");

    assert!(matches!(err, LowerError::Unsupported(_)));
}

#[test]
fn rejects_corrupt_decimal_instead_of_silent_fallback() {
    let err = LensValue::Decimal {
        value: "not-a-number".to_string(),
        precision: 12,
        scale: 2,
    }
    .lower_for_redaction()
    .expect_err("corrupt decimal should be rejected");

    assert!(matches!(
        err,
        LowerError::Decode {
            kind: "decimal",
            ..
        }
    ));
}
