use gaze_lens_protocol::{bounds, wire::*};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst};
static ACTIVE: AtomicBool = AtomicBool::new(false);
static TOTAL: AtomicUsize = AtomicUsize::new(0);
static MAX: AtomicUsize = AtomicUsize::new(0);
struct Meter;
fn record(size: usize) {
    if ACTIVE.load(SeqCst) {
        TOTAL.fetch_add(size, SeqCst);
        MAX.fetch_max(size, SeqCst);
    }
}
// SAFETY: the meter delegates each allocation/deallocation unchanged to System.
unsafe impl GlobalAlloc for Meter {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        record(l.size());
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        record(n);
        unsafe { System.realloc(p, l, n) }
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        record(l.size());
        unsafe { System.alloc_zeroed(l) }
    }
}
#[global_allocator]
static METER: Meter = Meter;
// A single test owns this process-wide meter. Fixture construction is excluded.
// Counts requested allocation sizes and cumulative traffic, not live/peak RSS.
fn measured(
    ceiling: usize,
    total_ceiling: usize,
    ok: bool,
    f: impl FnOnce() -> gaze_lens_protocol::Result<()>,
) -> (usize, usize) {
    TOTAL.store(0, SeqCst);
    MAX.store(0, SeqCst);
    ACTIVE.store(true, SeqCst);
    let r = f();
    ACTIVE.store(false, SeqCst);
    let sizes = (MAX.load(SeqCst), TOTAL.load(SeqCst));
    assert_eq!(r.is_ok(), ok, "{r:?}");
    assert!(
        sizes.0 <= ceiling && sizes.1 <= total_ceiling,
        "allocation requests: {sizes:?}"
    );
    sizes
}
const ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
fn binding() -> String {
    format!(
        r#"{{"principal":"{}","principal_generation":"{}","resource":"{}","resource_generation":"{}"}}"#,
        "a".repeat(32),
        "b".repeat(32),
        "c".repeat(32),
        "d".repeat(32)
    )
}
fn main() {
    // Input construction happens before metering. No producer or network access.
    let raw_scalar = format!("\"{}\"", "x".repeat(49_000));
    measured(0, 0, false, || bounds::source_json(raw_scalar.as_bytes()));
    let escaped = format!("\"{}\"", "\\u0061".repeat(8193));
    measured(0, 0, false, || bounds::source_json(escaped.as_bytes()));
    let request = format!(
        "{{\"version\":\"{VERSION}\",\"id\":\"{ID}\",\"operation\":\"query\",\"binding\":{},\"args\":{{\"table\":\"t\"{} }}}}\n",
        binding(),
        " ".repeat(900_000)
    );
    measured(1024, 2048, false, || {
        decode_call(request.as_bytes()).map(|_| ())
    });
    let c=decode_call(format!("{{\"version\":\"{VERSION}\",\"id\":\"{ID}\",\"operation\":\"readiness\",\"binding\":{},\"args\":{{}}}}\n",binding()).as_bytes()).unwrap();
    let response = format!(
        "{{\"version\":\"{VERSION}\",\"id\":\"{ID}\",\"operation\":\"readiness\",\"binding\":{},\"result\":{{\"kind\":\"readiness\",\"status\":\"configured\"{} }}}}\n",
        binding(),
        " ".repeat(900_000)
    );
    measured(1024, 2048, false, || {
        decode_success(response.as_bytes(), &c).map(|_| ())
    });
    let oversized_binding = format!(
        "{{\"version\":\"{VERSION}\",\"privacy\":\"client_gaze\",\"id\":\"{ID}\",\"operation\":\"readiness\",\"binding\":{{\"principal\":\"{}\",\"principal_generation\":\"{}\",\"resource\":\"{}\",\"resource_generation\":\"{}\"}}}}\n",
        "a".repeat(3000),
        "b".repeat(32),
        "c".repeat(32),
        "d".repeat(32)
    );
    measured(1024, 1024, false, || {
        decode_prepared(oversized_binding.as_bytes()).map(|_| ())
    });

    // Every binding field admits 32 decoded bytes. The rejection traffic for
    // 33 and 3000 bytes must match, including escaped input and late fields.
    let valid_binding = oversized_binding.replace(&"a".repeat(3000), &"a".repeat(32));
    for letter in ["a", "b", "c", "d"] {
        for escaped in [false, true] {
            let unit = if escaped {
                format!("\\u00{:02x}", letter.as_bytes()[0])
            } else {
                letter.into()
            };
            let at_cap = valid_binding.replace(&letter.repeat(32), &unit.repeat(32));
            measured(1024, 2048, true, || {
                decode_prepared(at_cap.as_bytes()).map(|_| ())
            });
            let near = valid_binding.replace(&letter.repeat(32), &unit.repeat(33));
            let far = valid_binding.replace(&letter.repeat(32), &unit.repeat(3000));
            measured(1024, 1024, false, || {
                decode_prepared(near.as_bytes()).map(|_| ())
            });
            // Escaped 3000 is over the handshake cap; keep this comparison in
            // a Call frame so field-specific admission, not framing, rejects.
            let as_call = |input: String| {
                input
                    .replace(",\"privacy\":\"client_gaze\"", "")
                    .replace("}}\n", "},\"args\":{}}\n")
            };
            let near = as_call(near);
            let far = as_call(far);
            let near_call_sizes = measured(1024, 1024, false, || {
                decode_call(near.as_bytes()).map(|_| ())
            });
            let far_call_sizes = measured(1024, 1024, false, || {
                decode_call(far.as_bytes()).map(|_| ())
            });
            assert_eq!(near_call_sizes, far_call_sizes);
        }
    }
    let key_at = format!("{{\"{}\":0}}", "\\u0061".repeat(bounds::SCALAR_BYTES));
    measured(16384, 32768, true, || {
        bounds::source_json(key_at.as_bytes())
    });
    let key_over = format!("{{\"{}\":0}}", "\\u0061".repeat(bounds::SCALAR_BYTES + 1));
    measured(0, 0, false, || bounds::source_json(key_over.as_bytes()));
    for unit in ["x", "é", "😀", "\\ud83d\\ude00"] {
        let decoded_len = if unit.starts_with('\\') {
            4
        } else {
            unit.len()
        };
        let at_cap = format!("\"{}\"", unit.repeat(bounds::SCALAR_BYTES / decoded_len));
        measured(0, 0, true, || bounds::source_json(at_cap.as_bytes()));
        let over = format!("\"{}x\"", unit.repeat(bounds::SCALAR_BYTES / decoded_len));
        measured(0, 0, false, || bounds::source_json(over.as_bytes()));
    }
    let outbound = Call {
        id: ID.into(),
        binding: c.binding.clone(),
        args: Args::LogRegex(Regex {
            pattern: "\0".repeat(bounds::REGEX_BYTES),
            level: Some("\0".repeat(bounds::SCALAR_BYTES)),
            limit: 1,
        }),
    };
    outbound.args.validate().unwrap();
    measured(1024, 2048, false, || encode_call(&outbound).map(|_| ()));
}
