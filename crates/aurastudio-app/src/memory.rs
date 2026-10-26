//! Memory report (Window > Memory report): measures what the process
//! holds, piece by piece, for `aurastudio_ui::MemoryReport`. Each piece
//! is named so a figure in Task Manager can be matched to a cause.
//!
//! Set AURASTUDIO_MEMLOG=1 to also print each report to the console.

use aurastudio_document::Document;
use aurastudio_gpu::GpuMemory;
use aurastudio_layers::Node;
use aurastudio_ui::{MemoryReport, MemorySection};

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

/// The document's pieces: tiles by layer, history, selection.
pub fn document_sections(doc: &Document) -> Vec<MemorySection> {
    let mut tiles = MemorySection::new(format!("Document \"{}\" {}x{}: tiles by layer", doc.name, doc.layers.width, doc.layers.height), true);
    for id in doc.layers.layer_ids() {
        if let Some(n @ Node::Layer(l)) = doc.layers.node(id) {
            tiles.add(format!("{} (#{id})", l.name), n.bytes() as u64);
        }
    }
    let mut other = MemorySection::new("Document: history and selection", true);
    other.add(format!("undo / redo history ({} + {} steps, compressed)", doc.history().undo_len(), doc.history().redo_len()), doc.history().bytes() as u64);
    other.add("selection", doc.selection.as_ref().map_or(0, |s| s.bytes() as u64));
    vec![tiles, other]
}

/// What the graphics side holds.
pub fn gpu_sections(g: &GpuMemory) -> Vec<MemorySection> {
    let mut known = MemorySection::new(format!("GPU: resources by size ({})", g.backend), false);
    for (n, b) in &g.known { known.add(n.clone(), *b); }
    let mut egui = MemorySection::new("GPU: interface textures (egui)", false);
    for (n, b) in &g.egui { egui.add(n.clone(), *b); }
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
    out
}

/// A report from its parts.
pub fn build(doc: Option<&Document>, patches_bytes: u64, gpu: Option<&GpuMemory>) -> MemoryReport {
    let (working_set, private) = process_memory();
    let mut sections = Vec::new();
    if let Some(d) = doc { sections.extend(document_sections(d)); }
    let mut buffers = MemorySection::new("Buffers", true);
    buffers.add("merged pieces waiting for the GPU", patches_bytes);
    sections.push(buffers);
    if let Some(g) = gpu { sections.extend(gpu_sections(g)); }
    MemoryReport { working_set, private, sections }
}

/// Print `r` if AURASTUDIO_MEMLOG is set.
pub fn log(r: &MemoryReport) {
    if std::env::var_os("AURASTUDIO_MEMLOG").is_some() { eprintln!("--- memory report ---\n{}", r.text()); }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aurastudio_document::{DocId, Document};

    #[test]
    fn every_layer_has_a_line_and_the_process_is_measured() {
        let mut doc = Document::new(DocId(1), "t", 300, 200);
        let a = doc.layers.active_id();
        doc.layers.set_layer_rect(a, (0, 0, 64, 64), &[255, 0, 0, 255].repeat(64 * 64));
        let r = build(Some(&doc), 1234, None);
        let tiles = &r.sections[0];
        assert!(tiles.title.contains("300x200"));
        assert_eq!(tiles.lines.len(), doc.layers.layer_count());
        assert!(tiles.lines.iter().any(|l| l.0.starts_with("Layer 1") && l.1 > 0), "{:?}", tiles.lines);
        assert!(r.text().contains("merged pieces waiting"));
        #[cfg(target_os = "linux")]
        assert!(r.working_set.unwrap() > 1 << 20, "{r:?}");
    }
}
