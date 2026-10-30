//! Tests: the heap bytes a piece of code keeps, counted per thread by a
//! global allocator that wraps the system one.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    /// Bytes allocated minus bytes freed by this thread.
    static LIVE: Cell<isize> = const { Cell::new(0) };
}

fn add(n: isize) {
    // `try_with`: a thread being torn down has no counter left.
    let _ = LIVE.try_with(|l| l.set(l.get() + n));
}

struct Counting;

// SAFETY: every call is passed to `System` unchanged; only a counter of
// this thread changes (no allocation happens in the counter).
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        add(layout.size() as isize);
        // SAFETY: the caller's contract is passed on.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        add(layout.size() as isize);
        // SAFETY: the caller's contract is passed on.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        add(-(layout.size() as isize));
        // SAFETY: the caller's contract is passed on.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        add(new_size as isize - layout.size() as isize);
        // SAFETY: the caller's contract is passed on.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

/// Runs `f` on this thread and returns its result with the bytes it
/// allocated on this thread and did not free: what the result keeps.
pub fn retained<R>(f: impl FnOnce() -> R) -> (R, usize) {
    let before = LIVE.with(Cell::get);
    let r = f();
    let after = LIVE.with(Cell::get);
    (r, after.saturating_sub(before).max(0) as usize)
}
