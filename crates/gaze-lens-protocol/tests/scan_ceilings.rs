use gaze_lens_protocol::wire::*;

#[test]
fn lower_scan_byte_ceiling_accepts_an_honest_truncation_reason() {
    let result = ResultBody::LogWindow {
        lines: vec!["x".into()],
        window: Window {
            scope: Scope::TailWindow,
            scanned_bytes: 4096,
            scanned_lines: 2,
            admitted_lines: 1,
        },
        truncated: vec![Reason::ScanBytes, Reason::Boundary],
    };
    assert!(result.validate_for(&Args::LogWindow(Empty {})).is_ok());
    let call = Call {
        id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".into(),
        binding: DestinationBinding {
            principal: "a".repeat(32),
            principal_generation: "b".repeat(32),
            resource: "c".repeat(32),
            resource_generation: "d".repeat(32),
        },
        args: Args::LogWindow(Empty {}),
    };
    let success = Success {
        id: call.id.clone(),
        operation: Operation::LogWindow,
        binding: call.binding.clone(),
        result,
    };
    let bytes = encode_success(&success, &call).unwrap();
    decode_success(&bytes, &call).unwrap();
}

#[test]
fn lower_scan_line_ceiling_accepts_an_honest_truncation_reason() {
    let result = ResultBody::LogWindow {
        lines: vec!["x".into()],
        window: Window {
            scope: Scope::TailWindow,
            scanned_bytes: 200,
            scanned_lines: 100,
            admitted_lines: 1,
        },
        truncated: vec![Reason::Lines, Reason::ScanLines],
    };
    assert!(result.validate_for(&Args::LogWindow(Empty {})).is_ok());
    let call = Call {
        id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".into(),
        binding: DestinationBinding {
            principal: "a".repeat(32),
            principal_generation: "b".repeat(32),
            resource: "c".repeat(32),
            resource_generation: "d".repeat(32),
        },
        args: Args::LogWindow(Empty {}),
    };
    let success = Success {
        id: call.id.clone(),
        operation: Operation::LogWindow,
        binding: call.binding.clone(),
        result,
    };
    let bytes = encode_success(&success, &call).unwrap();
    decode_success(&bytes, &call).unwrap();
}

#[test]
fn lower_ceilings_preserve_hard_caps_and_counter_reason_checks() {
    use gaze_lens_protocol::bounds;
    for (bytes, scanned, admitted, reasons) in [
        (bounds::SCAN_BYTES + 1, 1, 1, vec![Reason::ScanBytes]),
        (
            bounds::SCAN_BYTES,
            bounds::SCAN_LINES + 1,
            1,
            vec![Reason::ScanLines],
        ),
        (2, 1, 2, vec![Reason::ScanLines]),
        (1, 2, 1, vec![Reason::ScanBytes]),
        (2, 1, 1, vec![Reason::ScanBytes, Reason::ScanBytes]),
        (2, 1, 1, vec![Reason::ScanLines, Reason::ScanBytes]),
        (2, 1, 1, vec![Reason::Rows]),
    ] {
        let result = ResultBody::LogWindow {
            lines: vec!["x".into()],
            window: Window {
                scope: Scope::TailWindow,
                scanned_bytes: bytes,
                scanned_lines: scanned,
                admitted_lines: admitted,
            },
            truncated: reasons,
        };
        assert!(result.validate_for(&Args::LogWindow(Empty {})).is_err());
    }
}
