// Compiled separately so only the test allocator contains unsafe code.
#![deny(unsafe_op_in_unsafe_fn)]

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
pub fn measured<E: std::fmt::Debug>(
    ceiling: usize,
    total_ceiling: usize,
    ok: bool,
    f: impl FnOnce() -> Result<(), E>,
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
