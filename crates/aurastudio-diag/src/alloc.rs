//! The counting allocator (diagnostic builds only): any allocator,
//! wrapped, counting the bytes alive, the peak, and every allocation by
//! the zone it was made in. Never in the release build.
//!
//! `#[global_allocator] static A: Counting<mimalloc::MiMalloc> = Counting(mimalloc::MiMalloc);`

use std::alloc::{GlobalAlloc, Layout};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use crate::record::{CURRENT, MAX_ZONES};

pub struct Counting<A>(pub A);

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static ALLOCS: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);
static ZONE_ALLOCS: [AtomicU64; MAX_ZONES] = [const { AtomicU64::new(0) }; MAX_ZONES];
static ZONE_BYTES: [AtomicU64; MAX_ZONES] = [const { AtomicU64::new(0) }; MAX_ZONES];
/// Whether a counting allocator is installed (else the heap lines say so).
static ACTIVE: AtomicUsize = AtomicUsize::new(0);

#[inline(always)]
fn counted(size: usize) {
    let live = LIVE.fetch_add(size, Ordering::Relaxed) + size;
    PEAK.fetch_max(live, Ordering::Relaxed);
    ALLOCS.fetch_add(1, Ordering::Relaxed);
    BYTES.fetch_add(size as u64, Ordering::Relaxed);
    // (`try_with`: a thread being torn down still allocates.)
    let z = CURRENT.try_with(|c| c.get()).unwrap_or(0) as usize;
    ZONE_ALLOCS[z].fetch_add(1, Ordering::Relaxed);
    ZONE_BYTES[z].fetch_add(size as u64, Ordering::Relaxed);
    if ACTIVE.load(Ordering::Relaxed) == 0 { ACTIVE.store(1, Ordering::Relaxed); }
}

// SAFETY: every call goes to the inner allocator unchanged; the counters
// only follow the sizes and never allocate.
unsafe impl<A: GlobalAlloc> GlobalAlloc for Counting<A> {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = self.0.alloc(l);
        if !p.is_null() { counted(l.size()); }
        p
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        let p = self.0.alloc_zeroed(l);
        if !p.is_null() { counted(l.size()); }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        self.0.dealloc(p, l);
        LIVE.fetch_sub(l.size(), Ordering::Relaxed);
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        let q = self.0.realloc(p, l, new);
        if !q.is_null() {
            LIVE.fetch_sub(l.size(), Ordering::Relaxed);
            counted(new);
        }
        q
    }
}

/// The memory at this moment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemoryNow {
    /// Heap bytes alive and their peak (None without the counting
    /// allocator).
    pub heap:          Option<u64>,
    pub heap_peak:     Option<u64>,
    /// Allocations and bytes allocated since the last reset.
    pub allocs:        u64,
    pub alloc_bytes:   u64,
    pub working_set:   Option<u64>,
    pub private_bytes: Option<u64>,
}

pub fn memory_now() -> MemoryNow {
    let on = ACTIVE.load(Ordering::Relaxed) != 0;
    let (ws, pb) = crate::os::process_memory();
    MemoryNow {
        heap:          on.then(|| LIVE.load(Ordering::Relaxed) as u64),
        heap_peak:     on.then(|| PEAK.load(Ordering::Relaxed) as u64),
        allocs:        ALLOCS.load(Ordering::Relaxed),
        alloc_bytes:   BYTES.load(Ordering::Relaxed),
        working_set:   ws,
        private_bytes: pb,
    }
}

/// (allocations, bytes) per zone index since the last reset.
pub(crate) fn per_zone() -> Vec<(u64, u64)> {
    (0..MAX_ZONES).map(|i| (ZONE_ALLOCS[i].load(Ordering::Relaxed), ZONE_BYTES[i].load(Ordering::Relaxed))).collect()
}

pub(crate) fn reset_counts() {
    ALLOCS.store(0, Ordering::Relaxed);
    BYTES.store(0, Ordering::Relaxed);
    PEAK.store(LIVE.load(Ordering::Relaxed), Ordering::Relaxed);
    for i in 0..MAX_ZONES {
        ZONE_ALLOCS[i].store(0, Ordering::Relaxed);
        ZONE_BYTES[i].store(0, Ordering::Relaxed);
    }
}
