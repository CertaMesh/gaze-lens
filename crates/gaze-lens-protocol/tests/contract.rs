use gaze_lens_protocol::{
    Error, bounds,
    query::{Operand, Scalar},
    value::{ExactJson, Value},
    wire::*,
};
use serde_json::{Value as Json, json};

const ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
fn binding() -> Json {
    json!({"principal":"a".repeat(32),"principal_generation":"b".repeat(32),"resource":"c".repeat(32),"resource_generation":"d".repeat(32)})
}
fn frame(v: Json) -> Vec<u8> {
    let mut v = serde_json::to_vec(&v).unwrap();
    v.push(b'\n');
    v
}
fn prepare(op: &str) -> Vec<u8> {
    frame(
        json!({"version":VERSION,"privacy":"client_gaze","id":ID,"credential":"AB".repeat(32),"resource":"db","operation":op}),
    )
}
fn prepared(op: &str) -> Vec<u8> {
    frame(
        json!({"version":VERSION,"privacy":"client_gaze","id":ID,"operation":op,"binding":binding()}),
    )
}
fn call(op: &str, args: Json) -> Call {
    decode_call(&frame(
        json!({"version":VERSION,"id":ID,"operation":op,"binding":binding(),"args":args}),
    ))
    .unwrap()
}
fn success(c: &Call, result: Json) -> Vec<u8> {
    frame(
        json!({"version":VERSION,"id":ID,"operation":c.args.operation(),"binding":binding(),"result":result}),
    )
}
fn roundtrip(c: &Call, r: Json) {
    let bytes = success(c, r);
    let s = decode_success(&bytes, c).unwrap();
    assert_eq!(
        serde_json::from_slice::<Json>(&encode_success(&s, c).unwrap()).unwrap(),
        serde_json::from_slice::<Json>(&bytes).unwrap()
    );
}
fn page_result(offset: usize, total: usize, n: usize, next: Option<usize>, reasons: Json) -> Json {
    json!({"kind":"inspection","view":"packages","collector":"dpkg","status":"ok","evidence":"dpkg_database","records":(0..n).map(|i|json!({"name":format!("package-{i:05}"),"version":"1.0","architecture":"arm64"})).collect::<Vec<_>>(),"page":{"offset":offset,"returned":n,"total":total,"next_offset":next},"truncated":reasons})
}
fn window() -> Json {
    json!({"scope":"tail_window","scanned_bytes":20,"scanned_lines":2,"admitted_lines":2})
}

#[test]
fn every_operation_and_result_pair_roundtrips() {
    let fixtures = [
        (
            "query",
            json!({"table":"users"}),
            json!({"kind":"query_rows","columns":["name"],"rows":[[{"kind":"string","value":"private"}]],"truncated":[]}),
        ),
        (
            "schema",
            json!({"table":"users"}),
            json!({"kind":"table_schema","table":"users","columns":[{"name":"id","data_type":"sensitive-type-canary","nullable":false}],"truncated":[]}),
        ),
        (
            "list_tables",
            json!({}),
            json!({"kind":"table_list","tables":["users"],"truncated":[]}),
        ),
        (
            "log_tail",
            json!({"lines":2}),
            json!({"kind":"log_window","lines":["first","second"],"window":window(),"truncated":[]}),
        ),
        (
            "log_window",
            json!({}),
            json!({"kind":"log_window","lines":["first","second"],"window":window(),"truncated":[]}),
        ),
        (
            "log_regex",
            json!({"pattern":"private","limit":1}),
            json!({"kind":"log_matches","lines":["first"],"window":window(),"matched_lines":2,"truncated":["lines"]}),
        ),
        (
            "inspect",
            json!({"view":"host","collector":"linux","limit":1}),
            json!({"kind":"inspection","view":"host","collector":"linux","status":"ok","evidence":"os_release","records":[{"os_id":"debian","version_id":null,"architecture":"arm64"}],"page":null,"truncated":[]}),
        ),
        (
            "readiness",
            json!({}),
            json!({"kind":"readiness","status":"configured"}),
        ),
    ];
    for (op, a, r) in fixtures {
        let c = call(op, a);
        let encoded = encode_call(&c).unwrap();
        assert!(decode_call(&encoded).is_ok());
        roundtrip(&c, r.clone());
        let other = call(
            if op == "readiness" {
                "list_tables"
            } else {
                "readiness"
            },
            json!({}),
        );
        assert!(decode_success(&success(&other, r), &other).is_err());
    }
}
#[test]
fn sequence_is_single_use_and_binding_changes_poison_it() {
    let mut s = Sequence::default();
    assert!(s.call(call("readiness", json!({}))).is_err());
    assert!(
        s.prepare(decode_prepare(&prepare("readiness")).unwrap())
            .is_err()
    );
    let mut s = Sequence::default();
    s.prepare(decode_prepare(&prepare("readiness")).unwrap())
        .unwrap();
    s.prepared(decode_prepared(&prepared("readiness")).unwrap())
        .unwrap();
    let c = call("readiness", json!({}));
    let response = success(&c, json!({"kind":"readiness","status":"configured"}));
    s.call(c).unwrap();
    s.success(&response).unwrap();
    assert!(s.success(&response).is_err());
    let mut s = Sequence::default();
    s.prepare(decode_prepare(&prepare("readiness")).unwrap())
        .unwrap();
    s.prepared(decode_prepared(&prepared("readiness")).unwrap())
        .unwrap();
    let mut c = call("readiness", json!({}));
    c.binding.principal_generation = "e".repeat(32);
    assert!(matches!(s.call(c), Err(Error::BindingChanged)));
    assert!(s.call(call("readiness", json!({}))).is_err());
}
#[test]
fn prepare_contains_no_executable_values_and_binding_fields_are_fixed() {
    for (field, value) in [
        ("args", json!({})),
        ("table", json!("private")),
        ("pattern", json!("secret")),
        ("terms", json!([])),
        ("details", json!("source")),
    ] {
        let mut p: Json = serde_json::from_slice(&prepare("query")).unwrap();
        p[field] = value;
        assert!(decode_prepare(&frame(p)).is_err());
    }
    for field in [
        "principal",
        "principal_generation",
        "resource",
        "resource_generation",
    ] {
        for bad in [
            "a".repeat(31),
            "a".repeat(33),
            "A".repeat(32),
            "é".repeat(16),
        ] {
            let mut p: Json = serde_json::from_slice(&prepared("query")).unwrap();
            p["binding"][field] = json!(bad);
            assert!(decode_prepared(&frame(p)).is_err());
        }
    }
    for id in [
        "8ZZZZZZZZZZZZZZZZZZZZZZZZZ",
        "01arz3ndektsv4rrffq69g5fav",
        "01ARZ3NDEKTSV4RRFFQ69G5FAI",
    ] {
        let mut p: Json = serde_json::from_slice(&prepare("query")).unwrap();
        p["id"] = json!(id);
        assert!(decode_prepare(&frame(p)).is_err());
    }
}
#[test]
fn unknown_and_duplicate_fields_deny_at_every_depth() {
    let c = call("query", json!({"table":"users"}));
    let valid = success(
        &c,
        json!({"kind":"query_rows","columns":["x"],"rows":[[{"kind":"json","value":{"dynamic":{"key":1}}}]],"truncated":[]}),
    );
    let s = String::from_utf8(valid).unwrap();
    for (from, to) in [
        (
            "\"version\":",
            "\"version\":\"gaze-lens-source/2\",\"version\":",
        ),
        (
            "\"principal\":",
            "\"principal\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\",\"principal\":",
        ),
        ("\"kind\":\"json\"", "\"kind\":\"json\",\"kind\":\"json\""),
        ("\"key\":1", "\"key\":1,\"k\\u0065y\":2"),
    ] {
        assert!(decode_success(s.replace(from, to).as_bytes(), &c).is_err());
    }
    for value in [
        json!({"kind":"null","extra":true}),
        json!({"kind":"i64","value":"1","allowed":true}),
        json!({"kind":"future","value":"x"}),
    ] {
        assert!(
            decode_success(
                &success(
                    &c,
                    json!({"kind":"query_rows","columns":["x"],"rows":[[value]],"truncated":[]})
                ),
                &c
            )
            .is_err()
        );
    }
    let schema = call("schema", json!({"table":"users"}));
    assert!(decode_success(&success(&schema,json!({"kind":"table_schema","table":"users","columns":[{"name":"id","data_type":"text","nullable":false,"allowed":true}],"truncated":[]})),&schema).is_err());
}
#[test]
fn typed_values_preserve_positive_matrix() {
    let values = [
        json!({"kind":"null"}),
        json!({"kind":"bool","value":true}),
        json!({"kind":"i64","value":i64::MIN.to_string()}),
        json!({"kind":"u64","value":u64::MAX.to_string()}),
        json!({"kind":"f64","value":"-0"}),
        json!({"kind":"f64","value":"1.25"}),
        json!({"kind":"f64","value":f64::MAX.to_string()}),
        json!({"kind":"string","value":"00123"}),
        json!({"kind":"bytes","base64":"AP8=","len":2}),
        json!({"kind":"uuid","value":"550e8400-e29b-41d4-a716-446655440000"}),
        json!({"kind":"datetime","semantic":"date","value":"2024-02-29"}),
        json!({"kind":"datetime","semantic":"time","value":"12:34:56.123456"}),
        json!({"kind":"datetime","semantic":"naive","value":"2024-02-29T12:34:56.123456"}),
        json!({"kind":"datetime","semantic":"zoned","value":"2024-02-29T12:34:56.123456+05:30"}),
    ];
    for v in values {
        let parsed: Value = serde_json::from_str(&v.to_string()).unwrap();
        assert_eq!(serde_json::to_value(parsed).unwrap(), v);
    }
    for (coefficient, scale, text, p, s) in [
        ("1", 3, "0.001", 3, 3),
        ("12300", 4, "1.2300", 5, 4),
        ("12", -2, "1200", 4, 0),
        ("0", -2, "0", 1, 0),
        ("-123", 2, "-1.23", 3, 2),
    ] {
        let v = Value::decimal(coefficient, scale).unwrap();
        assert_eq!(
            serde_json::to_value(&v).unwrap(),
            json!({"kind":"decimal","value":text,"precision":p,"scale":s})
        );
        v.validate().unwrap();
    }
    assert!(Value::decimal("12", i64::MIN).is_err());
    assert!(Value::decimal("12", i64::MAX).is_err());
}
#[test]
fn exact_json_lexemes_and_dynamic_private_keys_survive() {
    let source = r#"{"9007199254740993":[9007199254740993,0.12345678901234567890123456789,1.2300e+1000],"$serde_json::private::Number":"not-a-number"}"#;
    let v: Value = serde_json::from_str(&format!(r#"{{"kind":"json","value":{source}}}"#)).unwrap();
    let serialized = serde_json::to_string(&v).unwrap();
    assert!(serialized.contains(source));
    let json = ExactJson::parse(source).unwrap();
    assert_eq!(json.as_str(), source);
    assert!(ExactJson::parse(r#"{"a":1,"\u0061":2}"#).is_err());
    let c = call("query", json!({"table":"t"}));
    let bytes = format!(
        "{{\"version\":\"{VERSION}\",\"id\":\"{ID}\",\"operation\":\"query\",\"binding\":{},\"result\":{{\"kind\":\"query_rows\",\"columns\":[\"x\"],\"rows\":[[{serialized}]],\"truncated\":[]}}}}\n",
        binding()
    );
    let out = encode_success(&decode_success(bytes.as_bytes(), &c).unwrap(), &c).unwrap();
    assert!(String::from_utf8(out).unwrap().contains(source));
}
#[test]
fn invalid_values_reject_instead_of_coercing() {
    for v in [
        json!({"kind":"i64","value":"01"}),
        json!({"kind":"i64","value":"-0"}),
        json!({"kind":"u64","value":"-1"}),
        json!({"kind":"u64","value":"18446744073709551616"}),
        json!({"kind":"f64","value":"NaN"}),
        json!({"kind":"f64","value":"1e400"}),
        json!({"kind":"f64","value":"1e-400"}),
        json!({"kind":"decimal","value":"0.001","precision":1,"scale":3}),
        json!({"kind":"decimal","value":"1e2","precision":3,"scale":0}),
        json!({"kind":"bytes","base64":"AP8","len":2}),
        json!({"kind":"bytes","base64":"AP9=","len":2}),
        json!({"kind":"bytes","base64":"AP8=","len":1}),
        json!({"kind":"datetime","semantic":"date","value":"2023-02-29"}),
        json!({"kind":"datetime","semantic":"time","value":"2024-01-01T12:00:00Z"}),
        json!({"kind":"datetime","semantic":"naive","value":"2024-01-01T12:00:00Z"}),
        json!({"kind":"datetime","semantic":"zoned","value":"2024-01-01T12:00:00"}),
        json!({"kind":"uuid","value":"550E8400-e29b-41d4-a716-446655440000"}),
    ] {
        assert!(
            serde_json::from_str::<Value>(&v.to_string()).is_err(),
            "{v}"
        );
    }
}
#[test]
fn query_preserves_restored_strings_and_exact_number_operands() {
    let raw = format!(
        r#"{{"version":"{VERSION}","id":"{ID}","operation":"query","binding":{},"args":{{"table":"t","where":[{{"col":"bigint","op":"in","val":["9223372036854775807",9007199254740993,0.123456789012345678901] }},{{"col":"text","op":"eq","val":"00123"}}]}}}}"#,
        binding()
    ) + "\n";
    let c = decode_call(raw.as_bytes()).unwrap();
    let Args::Query(q) = &c.args else { panic!() };
    let ps = q.r#where.as_ref().unwrap();
    let Some(Operand::List(xs)) = &ps[0].val else {
        panic!()
    };
    assert!(matches!(&xs[0],Scalar::String(s) if s=="9223372036854775807"));
    let encoded = String::from_utf8(encode_call(&c).unwrap()).unwrap();
    assert!(encoded.contains("0.123456789012345678901"));
    assert!(encoded.contains("9007199254740993"));
    assert!(encoded.contains("00123"));
}
#[test]
fn invalid_query_grammar_and_package_only_offsets_deny() {
    for a in [
        json!({"table":"t","offset":0}),
        json!({"table":"t","sql":"select 1"}),
        json!({"table":"t","limit":0}),
        json!({"table":"t","limit":1001}),
        json!({"table":"t","columns":["x","x"]}),
        json!({"table":"t","where":[{"col":"x","op":"eq","value":1}]}),
        json!({"table":"t","where":[{"col":"x","op":"is_null","val":null}]}),
        json!({"table":"t","where":[{"col":"x","op":"in","val":[]}]}),
        json!({"table":"t","where":[{"col":"x","op":"in","val":[1,null]}]}),
        json!({"table":"t","where":[{"col":"x","op":"eq","val":{"x":1}}]}),
    ] {
        assert!(
            decode_call(&frame(
                json!({"version":VERSION,"id":ID,"operation":"query","binding":binding(),"args":a})
            ))
            .is_err()
        );
    }
    for a in [
        json!({"view":"host","collector":"linux","limit":1,"offset":0}),
        json!({"view":"php","collector":"php","limit":32,"offset":0}),
        json!({"view":"packages","collector":"dpkg","limit":500,"offset":100001}),
        json!({"view":"packages","collector":"dpkg","limit":500,"offset":null}),
    ] {
        assert!(decode_call(&frame(json!({"version":VERSION,"id":ID,"operation":"inspect","binding":binding(),"args":a}))).is_err());
    }
}
#[test]
fn package_pages_progress_past_500_and_validate_counts() {
    for (offset, total, n, next, reasons) in [
        (0, 600, 500, Some(500), json!(["records"])),
        (500, 600, 100, None, json!([])),
        (10, 100, 2, Some(12), json!(["bytes"])),
        (100, 100, 0, None, json!([])),
        (100000, 0, 0, None, json!([])),
    ] {
        let c = call(
            "inspect",
            json!({"view":"packages","collector":"dpkg","limit":500,"offset":offset}),
        );
        roundtrip(&c, page_result(offset, total, n, next, reasons));
    }
    let c = call(
        "inspect",
        json!({"view":"packages","collector":"dpkg","limit":500,"offset":0}),
    );
    for r in [
        page_result(0, 100, 0, Some(0), json!(["bytes"])),
        page_result(1, 100, 1, Some(2), json!(["bytes"])),
        page_result(0, 100, 1, Some(2), json!(["bytes"])),
        page_result(0, 100, 1, Some(1), json!(["rows"])),
        page_result(0, 100001, 1, Some(1), json!(["bytes"])),
        page_result(0, 100, 1, None, json!([])),
    ] {
        assert!(decode_success(&success(&c, r), &c).is_err());
    }
    let mut r = page_result(0, 2, 2, None, json!([]));
    r["records"][1] = r["records"][0].clone();
    assert!(decode_success(&success(&c, r), &c).is_err());
}
#[test]
fn fixed_keyword_window_never_accepts_terms_level_or_limit() {
    for args in [
        json!({"terms":["token"]}),
        json!({"level":"error"}),
        json!({"limit":1}),
        json!({"lines":10000}),
    ] {
        assert!(decode_call(&frame(json!({"version":VERSION,"id":ID,"operation":"log_window","binding":binding(),"args":args}))).is_err());
    }
    let c = call("log_window", json!({}));
    let r = json!({"kind":"log_window","lines":vec!["";1000],"window":{"scope":"tail_window","scanned_bytes":1000,"scanned_lines":1000,"admitted_lines":1000},"truncated":[]});
    roundtrip(&c, r.clone());
    for (field, v) in [
        ("admitted_lines", json!(999)),
        ("scanned_lines", json!(999)),
        ("scanned_bytes", json!(999)),
    ] {
        let mut r = r.clone();
        r["window"][field] = v;
        assert!(decode_success(&success(&c, r), &c).is_err());
    }
    for reasons in [
        json!(["rows"]),
        json!(["bytes", "bytes"]),
        json!(["boundary", "bytes"]),
        json!(["scan_bytes"]),
    ] {
        let mut r = r.clone();
        r["truncated"] = reasons;
        assert!(decode_success(&success(&c, r), &c).is_err());
    }
}
#[test]
fn framing_depth_nodes_and_scalar_caps_are_checked() {
    let valid = prepare("query");
    assert!(decode_prepare(&valid[..valid.len() - 1]).is_err());
    let mut two = valid.clone();
    two.extend(&valid);
    assert!(decode_prepare(&two).is_err());
    assert!(decode_prepare(b"[]\n").is_err());
    let mut b = bounds::FrameBuffer::new(valid.len()).unwrap();
    for chunk in valid.chunks(3) {
        b.push(chunk).unwrap();
    }
    assert_eq!(b.finish().unwrap(), valid);
    let mut b = bounds::FrameBuffer::new(valid.len() - 1).unwrap();
    assert!(matches!(b.push(&valid), Err(Error::CapExceeded)));
    assert!(
        bounds::json(
            format!("{}0{}", "[".repeat(31), "]".repeat(31)).as_bytes(),
            4096
        )
        .is_ok()
    );
    assert!(
        bounds::json(
            format!("{}0{}", "[".repeat(32), "]".repeat(32)).as_bytes(),
            4096
        )
        .is_err()
    );
    let nodes = format!("[{}]", vec!["0"; bounds::MAX_NODES - 1].join(","));
    assert!(bounds::json(nodes.as_bytes(), bounds::FRAME_BYTES).is_ok());
    let over = nodes.replacen('[', "[0,", 1);
    assert!(bounds::json(over.as_bytes(), bounds::FRAME_BYTES).is_err());
    let value = json!({"kind":"string","value":"x".repeat(8192)});
    assert!(serde_json::from_str::<Value>(&value.to_string()).is_ok());
    let value = json!({"kind":"string","value":"x".repeat(8193)});
    assert!(serde_json::from_str::<Value>(&value.to_string()).is_err());
    use base64::Engine;
    let value = json!({"kind":"bytes","base64":base64::engine::general_purpose::STANDARD.encode(vec![255;8192]),"len":8192});
    assert!(serde_json::from_str::<Value>(&value.to_string()).is_ok());
    assert_eq!(bounds::serialized_size(&"\n", 4).unwrap(), 4);
    assert!(bounds::serialized_size(&"\n", 3).is_err());
}
#[test]
fn all_fixed_failures_correlate_without_payloads() {
    for code in [
        "invalid_request",
        "unauthorized",
        "unsupported_version",
        "unsupported_operation",
        "unavailable",
        "cap_exceeded",
        "timeout",
        "binding_changed",
        "internal_failure",
    ] {
        let v = frame(json!({"version":VERSION,"id":ID,"code":code}));
        assert!(decode_failure(&v, ID).is_ok());
        let mut v: Json = serde_json::from_slice(&v).unwrap();
        v["message"] = json!("private");
        assert!(decode_failure(&frame(v), ID).is_err());
    }
    assert!(
        decode_failure(
            &frame(json!({"version":VERSION,"id":ID,"code":"future"})),
            ID
        )
        .is_err()
    );
}

#[test]
fn php_observations_are_explicit_and_unknown_is_not_success() {
    let c = call(
        "inspect",
        json!({"view":"php","collector":"php","limit":32}),
    );
    let r = json!({"kind":"inspection","view":"php","collector":"php","status":"ok","evidence":"configured_php","records":[{"runtime_id":"app","installed_package":{"status":"present","name":"php8.3-cli","version":"8.3.1"},"cli":{"status":"observed","version":"8.3.2"},"fpm":{"status":"observed","version":"8.2.0","active":true,"app_binding":"unknown"}}],"page":null,"truncated":[]});
    roundtrip(&c, r.clone());
    for (field, value) in [
        (
            "installed_package",
            json!({"status":"absent","name":"php","version":null}),
        ),
        ("cli", json!({"status":"unknown","version":"8.4"})),
        (
            "fpm",
            json!({"status":"unknown","version":null,"active":true,"app_binding":"proven"}),
        ),
    ] {
        let mut bad = r.clone();
        bad["records"][0][field] = value;
        assert!(decode_success(&success(&c, bad), &c).is_err());
    }
    for status in ["unavailable", "unsupported", "unknown"] {
        roundtrip(
            &c,
            json!({"kind":"inspection","view":"php","collector":"php","status":status,"evidence":"none","records":[],"page":null,"truncated":[]}),
        );
    }
    let mut bad = r.clone();
    bad["status"] = json!("unavailable");
    assert!(decode_success(&success(&c, bad), &c).is_err());
    let mut bad = r.clone();
    bad["records"][0]["cli"]
        .as_object_mut()
        .unwrap()
        .remove("version");
    assert!(decode_success(&success(&c, bad), &c).is_err());
    let mut bad = r;
    bad.as_object_mut().unwrap().remove("page");
    assert!(decode_success(&success(&c, bad), &c).is_err());
}
#[test]
fn response_correlation_shape_and_binding_mutations_fail() {
    let c = call("query", json!({"table":"t","limit":1}));
    let r = json!({"kind":"query_rows","columns":["a"],"rows":[[{"kind":"null"}]],"truncated":[]});
    let bytes = success(&c, r.clone());
    for (field, value) in [
        ("id", json!("01ARZ3NDEKTSV4RRFFQ69G5FAW")),
        ("version", json!("2025-11-25")),
        ("operation", json!("schema")),
        (
            "binding",
            json!({"principal":"e".repeat(32),"principal_generation":"b".repeat(32),"resource":"c".repeat(32),"resource_generation":"d".repeat(32)}),
        ),
    ] {
        let mut bad: Json = serde_json::from_slice(&bytes).unwrap();
        bad[field] = value;
        assert!(decode_success(&frame(bad), &c).is_err());
    }
    for (field, value) in [
        ("columns", json!(["a", "a"])),
        ("rows", json!([[{"kind":"null"},{"kind":"null"}]])),
        ("rows", json!([[{"kind":"null"}],[{"kind":"null"}]])),
        ("truncated", json!(["records"])),
    ] {
        let mut bad = r.clone();
        bad[field] = value;
        assert!(decode_success(&success(&c, bad), &c).is_err());
    }
}
#[test]
fn request_and_handshake_exact_byte_caps() {
    let p = prepare("query");
    let mut at_cap = p[..p.len() - 2].to_vec();
    at_cap.extend(vec![b' '; bounds::PREPARE_BYTES - p.len()]);
    at_cap.extend(b"}\n");
    assert_eq!(at_cap.len(), bounds::PREPARE_BYTES);
    assert!(decode_prepare(&at_cap).is_ok());
    at_cap.insert(at_cap.len() - 2, b' ');
    assert!(matches!(decode_prepare(&at_cap), Err(Error::CapExceeded)));
    let args = r#"{"table":"t"}"#;
    let padded = format!(
        "{}{} }}",
        &args[..args.len() - 1],
        " ".repeat(bounds::REQUEST_BYTES - args.len() - 1)
    );
    assert_eq!(padded.len(), bounds::REQUEST_BYTES);
    let request = |args: &str| {
        format!(
            r#"{{"version":"{VERSION}","id":"{ID}","operation":"query","binding":{},"args":{args}}}"#,
            binding()
        ) + "\n"
    };
    assert!(decode_call(request(&padded).as_bytes()).is_ok());
    let more = padded.replacen('}', " }", 1);
    assert!(matches!(
        decode_call(request(&more).as_bytes()),
        Err(Error::CapExceeded)
    ));
    let mut b = bounds::FrameBuffer::new(3).unwrap();
    assert!(b.push(b"1234").is_err());
    assert!(b.push(b"\n").is_err());
    assert!(b.finish().is_err());
    for v in ["null", "false", "[]"] {
        let raw = request(&format!(r#"{{"table":"t","limit":{v}}}"#));
        assert!(decode_call(raw.as_bytes()).is_err());
    }
}
#[test]
fn float_roundtrip_rejects_lost_precision_and_retains_equivalent_spellings() {
    for s in [
        "1.2500",
        "125e-2",
        "5e-324",
        "-0.0",
        "1.7976931348623157e308",
    ] {
        let v = format!(r#"{{"kind":"f64","value":"{s}"}}"#);
        assert!(serde_json::from_str::<Value>(&v).is_ok(), "{s}");
    }
    for s in ["0.10000000000000001", "9007199254740993", "1e-400"] {
        let v = format!(r#"{{"kind":"f64","value":"{s}"}}"#);
        assert!(serde_json::from_str::<Value>(&v).is_err(), "{s}");
    }
}

#[test]
fn lower_operator_ceilings_can_truncate_below_requested_limits() {
    let c = call("query", json!({"table":"t","limit":100}));
    roundtrip(
        &c,
        json!({"kind":"query_rows","columns":["x"],"rows":[[{"kind":"null"}]],"truncated":["rows"]}),
    );
    let c = call("log_regex", json!({"pattern":"x","limit":100}));
    roundtrip(
        &c,
        json!({"kind":"log_matches","lines":["x"],"matched_lines":2,"window":window(),"truncated":["lines"]}),
    );
    let c = call(
        "inspect",
        json!({"view":"packages","collector":"dpkg","limit":500}),
    );
    roundtrip(&c, page_result(0, 100, 1, Some(1), json!(["records"])));
}
