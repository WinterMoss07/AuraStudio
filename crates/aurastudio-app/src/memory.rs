//! Memory report (Window > Memory report): measures what the process
//! holds, piece by piece, for `aurastudio_ui::MemoryReport`. Each piece
//! is named so a figure in Task Manager can be matched to a cause.
//!
//! Set AURASTUDIO_MEMLOG=1 to also print each report to the console.

use aurastudio_document::Document;
use aurastudio_gpu::GpuMemory;
use crate::Locked;

/// Interface textures sent whole and freed since the start (for the
/// memory report: a churn shows here even when the textures alive are few).
#[derive(Clone, Copy, Debug, Default)]
pub struct TextureTraffic {
    pub made:        u64,
    pub made_bytes:  u64,
    pub freed:       u64,
    pub freed_bytes: u64,
    pub live_bytes:  u64,
    pub peak_bytes:  u64,
}

/// What the graphics side holds: the GPU's resources, and the interface's
/// textures (name, bytes) with their traffic.
pub struct GpuReport {
    pub gpu:     GpuMemory,
    pub egui:    Vec<(String, u64)>,
    pub traffic: TextureTraffic,
}
use aurastudio_ui::{MemoryReport, MemorySection};

use std::alloc::{GlobalAlloc, Layout};

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

/// Where the app's allocations go: the system's allocator (by default)
/// or mimalloc (feature `mimalloc`). Chosen when building, so the two
/// can be compared (README, Profiling).
#[cfg(feature = "mimalloc")]
use mimalloc::MiMalloc as Inner;
#[cfg(not(feature = "mimalloc"))]
use std::alloc::System as Inner;

/// The allocator's name, for the report.
pub const ALLOCATOR: &str = if cfg!(feature = "mimalloc") { "mimalloc" } else { "system" };

/// The global allocator: `Inner`, counting the bytes alive. Tells the
/// app's data from the driver's and the system's in the report.
pub struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every call goes to `Inner` unchanged; the counter only follows
// the sizes.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = Inner.alloc(l);
        if !p.is_null() { LIVE.fetch_add(l.size(), Ordering::Relaxed); }
        p
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        let p = Inner.alloc_zeroed(l);
        if !p.is_null() { LIVE.fetch_add(l.size(), Ordering::Relaxed); }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        Inner.dealloc(p, l);
        LIVE.fetch_sub(l.size(), Ordering::Relaxed);
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        let q = Inner.realloc(p, l, new);
        if !q.is_null() {
            LIVE.fetch_add(new, Ordering::Relaxed);
            LIVE.fetch_sub(l.size(), Ordering::Relaxed);
        }
        q
    }
}

/// Bytes the app's allocations hold now.
pub fn heap_bytes() -> u64 { LIVE.load(Ordering::Relaxed) as u64 }

/// Memory at the steps of the start: (step, private bytes, heap bytes).
static BOOT: Mutex<Vec<(&'static str, u64, u64)>> = Mutex::new(Vec::new());

/// Memory at the events of the session: (event, private bytes after it).
static EVENTS: Mutex<Vec<(&'static str, u64)>> = Mutex::new(Vec::new());

/// Note the memory after an event (a document made or closed, a file
/// opened, playback started / stopped). The last 40 are kept.
pub fn event(name: &'static str) {
    let mut e = EVENTS.locked();
    e.push((name, process_memory().1.unwrap_or(0)));
    if e.len() > 40 { e.remove(0); }
}

/// The events as a section: what each added to the private bytes since
/// the one before it (never below zero: a release shows as 0, and the next
/// line starts from the lower figure).
fn events_section() -> MemorySection {
    let e = EVENTS.locked();
    let mut sec = MemorySection::timeline("Events: private bytes added since the one before");
    let mut prev = None;
    for (name, p) in e.iter() {
        let line = match prev {
            None => format!("{name} (first: {})", aurastudio_ui::mb(*p)),
            Some(q) if *p >= q => format!("{name} (+)"),
            Some(q) => format!("{name} (released {})", aurastudio_ui::mb(q - *p)),
        };
        sec.add(line, prev.map_or(0, |q: u64| p.saturating_sub(q)));
        prev = Some(*p);
    }
    sec
}

/// Note the memory now as step `name` (once per name).
pub fn mark(name: &'static str) {
    let mut b = BOOT.locked();
    if b.iter().any(|x| x.0 == name) { return; }
    b.push((name, process_memory().1.unwrap_or(0), heap_bytes()));
}

/// The steps as a section: what each added to the private bytes, and to
/// the heap.
fn boot_section() -> MemorySection {
    let b = BOOT.locked();
    let mut sec = MemorySection::timeline("Startup: private bytes added by each step");
    let (mut prev_p, mut prev_h) = (0u64, 0u64);
    for (i, (name, p, h)) in b.iter().enumerate() {
        if i == 0 {
            sec.add(format!("{name} (the whole start, heap {})", aurastudio_ui::mb(*h)), *p);
        } else {
            sec.add(format!("{name} (heap {}{})", if *h >= prev_h { "+" } else { "-" }, aurastudio_ui::mb(h.abs_diff(prev_h))), p.saturating_sub(prev_p));
        }
        prev_p = *p;
        prev_h = *h;
    }
    sec
}

/// (working set, private bytes) of this process, as the system counts.
#[cfg(windows)]
pub fn process_memory() -> (Option<u64>, Option<u64>) {
    use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX};
    use windows::Win32::System::Threading::GetCurrentProcess;
    let mut c = PROCESS_MEMORY_COUNTERS_EX::default();
    c.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
    // SAFETY: `c` is a PROCESS_MEMORY_COUNTERS_EX (it starts with a
    // PROCESS_MEMORY_COUNTERS) and `cb` says how big it is.
    let ok = unsafe { GetProcessMemoryInfo(GetCurrentProcess(), (&mut c as *mut PROCESS_MEMORY_COUNTERS_EX).cast::<PROCESS_MEMORY_COUNTERS>(), c.cb) };
    if ok.is_ok() { (Some(c.WorkingSetSize as u64), Some(c.PrivateUsage as u64)) } else { (None, None) }
}

/// (resident set, data + stack) from /proc: the same two figures.
#[cfg(target_os = "linux")]
pub fn process_memory() -> (Option<u64>, Option<u64>) {
    let Ok(s) = std::fs::read_to_string("/proc/self/statm") else { return (None, None) };
    let f: Vec<u64> = s.split_whitespace().filter_map(|x| x.parse().ok()).collect();
    // (pages: size resident shared text lib data dt)
    (f.get(1).map(|p| p * 4096), f.get(5).map(|p| p * 4096))
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn process_memory() -> (Option<u64>, Option<u64>) { (None, None) }

/// (committed, in use) of the Windows process heap: the graphics driver's
/// and the system libraries' allocations, plus the app's own with the
/// system allocator (counted as the Rust heap either way). Committed minus in use: freed but kept.
#[cfg(windows)]
fn system_heap() -> Option<(u64, u64)> {
    use windows::Win32::System::Memory::{GetProcessHeap, HeapSummary, HEAP_SUMMARY};
    // SAFETY: the process heap handle is valid for the process's life; the
    // summary is a plain struct whose size is given in `cb`.
    unsafe {
        let heap = GetProcessHeap().ok()?;
        let mut s = HEAP_SUMMARY { cb: std::mem::size_of::<HEAP_SUMMARY>() as u32, ..Default::default() };
        HeapSummary(heap, 0, &mut s).as_bool().then_some((s.cbCommitted as u64, s.cbAllocated as u64))
    }
}

#[cfg(not(windows))]
fn system_heap() -> Option<(u64, u64)> { None }

/// The document's pieces: tiles by layer, history, selection.
pub fn document_sections(doc: &Document) -> Vec<MemorySection> {
    let s = doc.stack();
    let mut tiles = MemorySection::new(format!("Document \"{}\" {}x{}: tiles by layer (full RGBA ones)", doc.name, s.width, s.height), true);
    let tile = s.tile as u64;
    for id in s.layer_ids() {
        if let Some(l) = s.layer(id) {
            let st = l.surface.stored();
            let name = format!("{} (#{id}): {} full, {} alpha, {} uniform, {} cold tiles", l.name, st.full, st.alpha, st.uniform, st.cold);
            tiles.add(name, (st.full as u64 * 4 + st.alpha as u64) * tile * tile);
        }
    }
    let mut other = MemorySection::new("Document: history and selection", true);
    other.add(format!("undo / redo history ({} + {} steps, compressed)", doc.history().undo_len(), doc.history().redo_len()), doc.history().bytes() as u64);
    other.add("selection", doc.selection().map_or(0, |s| s.to_bytes().len() as u64));
    vec![tiles, other]
}

/// What the graphics side holds.
pub fn gpu_sections(r: &GpuReport) -> Vec<MemorySection> {
    let g = &r.gpu;
    let mut known = MemorySection::new(format!("GPU: resources by size ({})", g.backend), false);
    for (n, b) in &g.known { known.add(n.clone(), *b); }
    let mut egui = MemorySection::new("GPU: interface textures (egui)", false);
    for (n, b) in &r.egui { egui.add(n.clone(), *b); }
    let mut out = vec![known, egui];
    if let (Some(a), Some(r)) = (g.allocated, g.reserved) {
        let mut alloc = MemorySection::new("GPU: the driver's allocator", false);
        alloc.add("given to resources", a);
        alloc.add("reserved from the device (>= the above)", r);
        // The biggest allocations by name (the rest summed).
        for (n, b) in g.allocations.iter().take(12) { alloc.add(format!("  {n}"), *b); }
        let rest: u64 = g.allocations.iter().skip(12).map(|x| x.1).sum();
        if rest > 0 { alloc.add(format!("  ({} smaller ones)", g.allocations.len().saturating_sub(12)), rest); }
        // (Lines below the first two are the detail of the first.)
        out.push(alloc);
    }
    if !g.blocks.is_empty() { out.push(blocks_section(&g.blocks)); }
    out
}

/// The interface's texture cache: each page, how full.
fn cache_section(pages: &[aurastudio_ui::PageUse]) -> MemorySection {
    let mut s = MemorySection::new("GPU: interface texture cache (icons, thumbnails, stroke previews)", false);
    for (i, p) in pages.iter().enumerate() {
        s.add(format!("page {}: {:.0} % filled, {} picture{}", i + 1, p.filled * 100.0, p.pictures, if p.pictures == 1 { "" } else { "s" }), p.bytes as u64);
    }
    s
}

/// The driver allocator's blocks: what each holds, the empty ones kept
/// (the reserve the system counts in the process's memory on a GPU that
/// shares it).
fn blocks_section(blocks: &[(u64, Vec<(String, u64)>)]) -> MemorySection {
    let empty = blocks.iter().filter(|b| b.1.is_empty()).count();
    let mut s = MemorySection::new(format!("GPU: the driver's memory blocks ({} blocks, {empty} empty)", blocks.len()), false);
    for (i, (size, held)) in blocks.iter().enumerate() {
        let used: u64 = held.iter().map(|a| a.1).sum();
        let what = if held.is_empty() {
            "empty, kept for reuse".to_string()
        } else {
            // The biggest names it holds.
            let mut names: Vec<&(String, u64)> = held.iter().collect();
            names.sort_by_key(|a| std::cmp::Reverse(a.1));
            let shown: Vec<String> = names.iter().take(3).map(|a| if a.0.is_empty() { "(unnamed)".into() } else { a.0.clone() }).collect();
            let more = if held.len() > 3 { format!(" and {} more", held.len() - 3) } else { String::new() };
            format!("{:.0} % used by {} resource{}: {}{more}", used as f64 * 100.0 / (*size).max(1) as f64, held.len(), if held.len() > 1 { "s" } else { "" }, shown.join(", "))
        };
        s.add(format!("block {}: {what}", i + 1), *size);
    }
    s
}

/// The paint thread's caches (as it last told) and the pixel pool.
fn paint_thread_section() -> MemorySection {
    use std::sync::atomic::Ordering::Relaxed;
    let h = &crate::paint_thread::HELD;
    let mut s = MemorySection::new("Paint thread: caches", true);
    s.add("stroke cache (the layers under / over the stroke, 16-bit, while painting)", h.stroke_cache.load(Relaxed));
    s.add("stroke coverage (the stroke under way)", h.stroke.load(Relaxed));
    s.add("last save's encodings (incremental saves)", h.saves.load(Relaxed));
    s
}

/// A report from its parts.
pub fn build(doc: Option<&Document>, patches_bytes: u64, gpu: Option<&GpuReport>) -> MemoryReport {
    let (working_set, private) = process_memory();
    let mut sections = Vec::new();
    if let Some(d) = doc { sections.extend(document_sections(d)); }
    let mut buffers = MemorySection::new("Buffers", true);
    buffers.add("merged pieces waiting for the GPU", patches_bytes);
    sections.push(buffers);
    sections.push(paint_thread_section());
    if let Some(g) = gpu { sections.extend(gpu_sections(g)); }
    let pages = aurastudio_ui::ui_cache_usage();
    if !pages.is_empty() { sections.push(cache_section(&pages)); }
    sections.push(boot_section());
    sections.push(events_section());
    let mut header = vec![format!("AuraStudio {}, allocator {ALLOCATOR}{}", env!("CARGO_PKG_VERSION"),
        if cfg!(feature = "profile") { ", Tracy build" } else { "" })];
    if let Some(r) = gpu {
        let g = &r.gpu;
        header.push(format!("Graphics: {} on {}", g.backend, if g.adapter.is_empty() { "?" } else { &g.adapter }));
        header.push(format!("Display: canvas quality {}, frames presented {}", g.quality, g.present));
        header.push(format!("Shaders: {}", g.shaders));
        let t = &r.traffic;
        let mb = aurastudio_ui::mb;
        header.push(format!(
            "Interface textures since the start: {} sent whole ({}), {} freed ({}); alive now {}, most at once {}; uploads waited {} times (staging kept under 4 MB)",
            t.made, mb(t.made_bytes), t.freed, mb(t.freed_bytes), mb(t.live_bytes), mb(t.peak_bytes), g.upload_waits,
        ));
    }
    if let Some((committed, used)) = system_heap() {
        header.push(format!(
            "Windows process heap (driver and system libraries{}): {} committed, {} in use ({} freed but kept)",
            if cfg!(feature = "mimalloc") { "" } else { ", and the app's allocations" },
            aurastudio_ui::mb(committed), aurastudio_ui::mb(used), aurastudio_ui::mb(committed.saturating_sub(used)),
        ));
    }
    MemoryReport { working_set, private, heap: Some(heap_bytes()), header, latency: crate::latency::report(), sections }
}

/// Print `r` if AURASTUDIO_MEMLOG is set.
pub fn log(r: &MemoryReport) {
    if std::env::var_os("AURASTUDIO_MEMLOG").is_some() { eprintln!("--- memory report ---\n{}", r.text()); }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aurastudio_core::DocType;
    use aurastudio_document::{DocId, Document};

    #[test]
    fn every_layer_has_a_line_and_the_process_is_measured() {
        let mut doc = Document::new(DocId(1), "t", 300, 200, DocType::Illustration);
        doc.apply(aurastudio_document::Edit::Fill { x: 1, y: 1, color: [255, 0, 0, 255], params: Default::default() });
        let r = build(Some(&doc), 1234, None);
        let tiles = &r.sections[0];
        assert!(tiles.title.contains("300x200"));
        assert_eq!(tiles.lines.len(), doc.stack().layer_count());
        assert!(tiles.lines.iter().any(|l| l.0.starts_with("Layer 1") && l.1 > 0), "{:?}", tiles.lines);
        assert!(r.text().contains("merged pieces waiting"));
        // Events and startup steps are sections of their own.
        event("test event one");
        event("test event two");
        let r2 = build(None, 0, None);
        let ev = r2.sections.iter().find(|s| s.title.starts_with("Events")).unwrap();
        assert!(ev.timeline && ev.lines.iter().any(|l| l.0.contains("test event two")), "{:?}", ev.lines);
        #[cfg(target_os = "linux")]
        assert!(r.working_set.unwrap() > 1 << 20, "{r:?}");
    }

    /// Each block says what it holds; an empty one says it is kept.
    #[test]
    fn the_blocks_show_what_they_hold() {
        let s = blocks_section(&[(32 << 20, vec![("canvas pages (full)".into(), 28 << 20)]), (64 << 20, vec![])]);
        assert!(s.title.contains("2 blocks, 1 empty"), "{}", s.title);
        assert!(s.lines[0].0.contains("88 % used by 1 resource: canvas pages (full)"), "{:?}", s.lines);
        assert!(s.lines[1].0.contains("empty, kept for reuse") && s.lines[1].1 == 64 << 20);
    }
}
