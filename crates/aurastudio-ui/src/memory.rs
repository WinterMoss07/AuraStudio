//! The memory report: every megabyte the app holds, named, so what Task
//! Manager shows can be matched against it. The app measures (it owns the
//! document, the paint thread and the GPU); this holds the numbers, writes
//! them out as text and shows them in a window (Window > Memory report).

use egui::{RichText, ScrollArea};

use crate::theme;

/// One group of figures: bytes by name.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Section {
    pub title: String,
    /// In the process's own memory (RAM), so counted against what the
    /// system says it holds; the GPU's is its own (VRAM, or shared).
    pub cpu:   bool,
    /// Not parts of a whole: steps in time, each the growth since the last.
    pub timeline: bool,
    pub lines: Vec<(String, u64)>,
}

impl Section {
    pub fn new(title: impl Into<String>, cpu: bool) -> Self { Self { title: title.into(), cpu, timeline: false, lines: Vec::new() } }

    /// A section of steps in time (growth by step), counted nowhere.
    pub fn timeline(title: impl Into<String>) -> Self { Self { title: title.into(), cpu: false, timeline: true, lines: Vec::new() } }

    pub fn add(&mut self, name: impl Into<String>, bytes: u64) { self.lines.push((name.into(), bytes)); }

    pub fn total(&self) -> u64 { self.lines.iter().map(|l| l.1).sum() }
}

/// Everything measured at one moment.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MemoryReport {
    /// What the system says the process holds: working set (what Task
    /// Manager lists) and private bytes (committed, its own).
    pub working_set: Option<u64>,
    pub private:     Option<u64>,
    /// What the app's own allocations hold now (Rust's heap), counted by
    /// the allocator: separates its data from the driver's and the system's.
    pub heap:        Option<u64>,
    /// Lines under the title: the graphics adapter and API.
    pub header:      Vec<String>,
    pub sections:    Vec<Section>,
}

/// "123.45 MB".
pub fn mb(bytes: u64) -> String { format!("{:.2} MB", bytes as f64 / (1u64 << 20) as f64) }

impl MemoryReport {
    /// Bytes named in the sections of the process's own memory.
    pub fn accounted(&self) -> u64 { self.sections.iter().filter(|s| s.cpu).map(Section::total).sum() }

    /// Without a heap figure: what the system holds that no line names
    /// (private bytes, else the working set, minus the named RAM); with it:
    /// the heap that no line names.
    pub fn not_itemised(&self) -> Option<i64> {
        let held = self.heap.or(self.private).or(self.working_set)?;
        Some(held as i64 - self.accounted() as i64)
    }

    /// Private bytes outside the Rust heap: the graphics driver, system
    /// libraries, thread stacks.
    pub fn outside_heap(&self) -> Option<i64> {
        Some(self.private? as i64 - self.heap? as i64)
    }

    /// The report as text (copied, logged): one line per figure.
    pub fn text(&self) -> String {
        let mut s = String::new();
        let line = |s: &mut String, name: &str, bytes: u64| s.push_str(&format!("  {:>12}  {name}\n", mb(bytes)));
        for h in &self.header { s.push_str(&format!("{h}\n")); }
        if !self.header.is_empty() { s.push('\n'); }
        s.push_str("Process (the system's figures)\n");
        if let Some(w) = self.working_set { line(&mut s, "working set (Task Manager: Memory)", w); }
        if let Some(p) = self.private { line(&mut s, "private bytes (committed)", p); }
        if let Some(h) = self.heap { line(&mut s, "Rust heap (the app's own allocations)", h); }
        for sec in &self.sections {
            let tag = if sec.timeline { " (growth by step)" } else if sec.cpu { "" } else { " (graphics memory)" };
            s.push_str(&format!("\n{}{tag}\n", sec.title));
            for (name, bytes) in &sec.lines { line(&mut s, name, *bytes); }
            if sec.lines.len() > 1 && !sec.timeline { line(&mut s, "total", sec.total()); }
        }
        s.push_str(&format!("\nNamed in RAM: {}\n", mb(self.accounted())));
        let signed = |d: i64| format!("{}{}", if d < 0 { "-" } else { "" }, mb(d.unsigned_abs()));
        if let Some(d) = self.not_itemised() {
            let what = if self.heap.is_some() { "Rust heap not itemised (heap minus named)" } else { "Not itemised (system's figure minus named)" };
            s.push_str(&format!("{what}: {}\n", signed(d)));
        }
        if let Some(d) = self.outside_heap() {
            s.push_str(&format!("Outside the Rust heap (private minus heap: driver, system libraries, stacks): {}\n", signed(d)));
        }
        s
    }
}

/// The report window: refreshed by the app while it is open.
pub fn window(ctx: &egui::Context, open: &mut bool, report: Option<&MemoryReport>) {
    let mut shown = *open;
    egui::Window::new("Memory report").open(&mut shown).default_width(520.0).default_height(460.0).show(ctx, |ui| {
        let Some(r) = report else {
            ui.label(RichText::new("Measuring…").color(theme::TEXT_WEAK));
            return;
        };
        let text = r.text();
        ui.horizontal(|ui| {
            if ui.button("Copy").on_hover_text("Copy the report as text").clicked() { ctx.copy_text(text.clone()); }
            ui.label(RichText::new("Updated once a second.").small().color(theme::TEXT_WEAK));
        });
        ui.separator();
        ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
            ui.add(egui::Label::new(RichText::new(&text).monospace().size(12.0)).extend());
        });
    });
    *open = shown;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_gap_is_what_no_line_names() {
        let mut ram = Section::new("Document", true);
        ram.add("Layer 1", 100 << 20);
        ram.add("History", 20 << 20);
        let mut vram = Section::new("GPU", false);
        vram.add("canvas", 300 << 20);
        let r = MemoryReport { working_set: Some(500 << 20), private: Some(400 << 20), heap: None, header: Vec::new(), sections: vec![ram, vram] };
        // (The GPU's own memory is not RAM: not counted.)
        assert_eq!(r.accounted(), 120 << 20);
        assert_eq!(r.not_itemised(), Some(280 << 20));
        let t = r.text();
        assert!(t.contains("100.00 MB  Layer 1") && t.contains("graphics memory") && t.contains("Not itemised"), "{t}");
        // With the heap counted: the gap is the heap's, the rest is outside it.
        let with_heap = MemoryReport { heap: Some(150 << 20), ..r.clone() };
        assert_eq!(with_heap.not_itemised(), Some(30 << 20));
        assert_eq!(with_heap.outside_heap(), Some(250 << 20));
        assert!(with_heap.text().contains("Outside the Rust heap"));
        // Named more than the system says: shown negative, not hidden.
        let over = MemoryReport { private: Some(10 << 20), sections: vec![{ let mut s = Section::new("x", true); s.add("a", 20 << 20); s }], ..Default::default() };
        assert_eq!(over.not_itemised(), Some(-(10 << 20)));
        assert!(over.text().contains("-10.00 MB"));
    }
}
