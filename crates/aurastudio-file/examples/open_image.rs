//! Time and heap peak of opening pictures as a stack:
//! `cargo run --release -p aurastudio-file --example open_image -- a.jpg b.png`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

/// The system allocator, counting the bytes held and their peak.
struct Peak;
static NOW: AtomicUsize = AtomicUsize::new(0);
static TOP: AtomicUsize = AtomicUsize::new(0);

fn grew(n: usize) { let now = NOW.fetch_add(n, Relaxed) + n; TOP.fetch_max(now, Relaxed); }

// SAFETY: every call goes to the system allocator unchanged.
unsafe impl GlobalAlloc for Peak {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 { grew(l.size()); unsafe { System.alloc(l) } }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) { NOW.fetch_sub(l.size(), Relaxed); unsafe { System.dealloc(p, l) } }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, size: usize) -> *mut u8 {
        if size > l.size() { grew(size - l.size()) } else { NOW.fetch_sub(l.size() - size, Relaxed); }
        unsafe { System.realloc(p, l, size) }
    }
}

#[global_allocator]
static ALLOC: Peak = Peak;

fn main() {
    for path in std::env::args().skip(1) {
        for _ in 0..3 {
            let base = NOW.load(Relaxed);
            TOP.store(base, Relaxed);
            let t = std::time::Instant::now();
            let stack = match aurastudio_file::import_image(std::path::Path::new(&path), 16384, 128) {
                Ok((_, s)) => s,
                Err(e) => { println!("{path}: {e}"); break; }
            };
            let ms = t.elapsed().as_secs_f64() * 1e3;
            let (peak, kept) = (TOP.load(Relaxed) - base, NOW.load(Relaxed) - base);
            drop(stack);
            println!("{path}: {ms:.1} ms, heap peak {:.1} MB, kept {:.1} MB", peak as f64 / 1e6, kept as f64 / 1e6);
        }
    }
}
