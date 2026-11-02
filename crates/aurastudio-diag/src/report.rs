//! One report for a diagnostic session: header, each scenario run (top
//! zones, allocations, counters, samples, memory), the suspects found, in
//! JSON (stable, for Claude and for comparing two reports) and HTML (one
//! page for a person).

use std::fmt::Write as _;

use crate::record::Snapshot;

/// One run of one scenario.
pub struct Run {
    pub scenario: String,
    /// 1, 2…: the same scenario is run twice to see drift.
    pub run:      u32,
    pub snap:     Snapshot,
}

/// Something the rules found worth a look, with a hint where to look.
#[derive(Clone, Debug, PartialEq)]
pub struct Suspect {
    pub scenario: String,
    pub rule:     &'static str,
    pub what:     String,
    pub hint:     &'static str,
}

/// Limits of the rules.
#[derive(Clone, Debug)]
pub struct Rules {
    /// A zone on the main or paint thread whose p99 is above this (ns)
    /// costs a frame.
    pub frame_ns:     u64,
    /// Zones that must not allocate in steady state (per dab, per frame).
    pub no_alloc:     Vec<&'static str>,
    /// Private bytes rising more than this between two identical runs.
    pub drift_bytes:  u64,
}

impl Default for Rules {
    fn default() -> Self { Self { frame_ns: 16_666_667, no_alloc: Vec::new(), drift_bytes: 10 << 20 } }
}

#[derive(Default)]
pub struct Report {
    /// (key, value): version, build, OS, CPU, GPU, backend…
    pub header:   Vec<(String, String)>,
    pub runs:     Vec<Run>,
    pub suspects: Vec<Suspect>,
}

impl Report {
    pub fn add_run(&mut self, scenario: &str, snap: Snapshot) {
        let run = self.runs.iter().filter(|r| r.scenario == scenario).count() as u32 + 1;
        self.runs.push(Run { scenario: scenario.to_owned(), run, snap });
    }

    /// A finding a scenario makes itself (idle wakeups, preview targets…).
    pub fn suspect(&mut self, scenario: &str, rule: &'static str, what: String, hint: &'static str) {
        self.suspects.push(Suspect { scenario: scenario.to_owned(), rule, what, hint });
    }

    /// Evaluate the general rules over every run.
    pub fn check(&mut self, rules: &Rules) {
        let mut found = Vec::new();
        for r in &self.runs {
            for z in &r.snap.zones {
                let hot_thread = z.thread == "main" || z.thread.contains("paint");
                if hot_thread && z.p99 > rules.frame_ns {
                    found.push(Suspect { scenario: r.scenario.clone(), rule: "slow zone", what: format!("`{}` on {}: p99 {} (limit {})", z.zone, z.thread, ms(z.p99), ms(rules.frame_ns)), hint: "split the work, move it off this thread, or make it incremental" });
                }
                if rules.no_alloc.contains(&z.zone.as_str()) && z.allocs > z.calls / 100 + 1 {
                    found.push(Suspect { scenario: r.scenario.clone(), rule: "allocates", what: format!("`{}`: {} allocations for {} calls", z.zone, z.allocs, z.calls), hint: "reuse a buffer or take it from the pool" });
                }
            }
        }
        // Drift: the second run of a scenario holding more than the first.
        for r in self.runs.iter().filter(|r| r.run > 1) {
            let Some(first) = self.runs.iter().find(|f| f.scenario == r.scenario && f.run == 1) else { continue };
            if let (Some(a), Some(b)) = (first.snap.memory.private_bytes, r.snap.memory.private_bytes) {
                if b > a + rules.drift_bytes {
                    found.push(Suspect { scenario: r.scenario.clone(), rule: "drift", what: format!("private bytes {} after run 1, {} after run {}", mb(a), mb(b), r.run), hint: "something is kept between identical runs: look at the memory owners" });
                }
            }
        }
        self.suspects.extend(found);
    }

    pub fn to_json(&self) -> String {
        let mut o = String::from("{\n  \"format\": 1,\n  \"header\": {");
        for (i, (k, v)) in self.header.iter().enumerate() {
            let _ = write!(o, "{}\n    {}: {}", if i > 0 { "," } else { "" }, js(k), js(v));
        }
        o.push_str("\n  },\n  \"runs\": [");
        for (i, r) in self.runs.iter().enumerate() {
            let m = &r.snap.memory;
            let _ = write!(o, "{}\n    {{\"scenario\": {}, \"run\": {}, \"span_ns\": {},", if i > 0 { "," } else { "" }, js(&r.scenario), r.run, r.snap.span);
            let _ = write!(o, "\n     \"memory\": {{\"heap\": {}, \"heap_peak\": {}, \"allocs\": {}, \"alloc_bytes\": {}, \"working_set\": {}, \"private_bytes\": {}}},", opt(m.heap), opt(m.heap_peak), m.allocs, m.alloc_bytes, opt(m.working_set), opt(m.private_bytes));
            o.push_str("\n     \"zones\": [");
            for (j, z) in r.snap.zones.iter().enumerate() {
                let _ = write!(o, "{}\n       {{\"zone\": {}, \"thread\": {}, \"calls\": {}, \"total_ns\": {}, \"self_ns\": {}, \"max_ns\": {}, \"p50_ns\": {}, \"p95_ns\": {}, \"p99_ns\": {}, \"allocs\": {}, \"alloc_bytes\": {}}}",
                    if j > 0 { "," } else { "" }, js(&z.zone), js(&z.thread), z.calls, z.total, z.self_t, z.max, z.p50, z.p95, z.p99, z.allocs, z.alloc_bytes);
            }
            o.push_str("],\n     \"counters\": {");
            for (j, (k, v)) in r.snap.counters.iter().enumerate() { let _ = write!(o, "{}{}: {}", if j > 0 { ", " } else { "" }, js(k), v); }
            o.push_str("},\n     \"samples\": {");
            for (j, (k, n, last, min, max, mean)) in r.snap.samples.iter().enumerate() {
                let _ = write!(o, "{}{}: {{\"n\": {n}, \"last\": {}, \"min\": {}, \"max\": {}, \"mean\": {}}}", if j > 0 { ", " } else { "" }, js(k), num(*last), num(*min), num(*max), num(*mean));
            }
            o.push_str("}}");
        }
        o.push_str("\n  ],\n  \"suspects\": [");
        for (i, s) in self.suspects.iter().enumerate() {
            let _ = write!(o, "{}\n    {{\"scenario\": {}, \"rule\": {}, \"what\": {}, \"hint\": {}}}", if i > 0 { "," } else { "" }, js(&s.scenario), js(s.rule), js(&s.what), js(s.hint));
        }
        o.push_str("\n  ]\n}\n");
        o
    }

    /// One page, no script: header, suspects, then per run the top 10 zones
    /// by self time, by p99 and by allocations, counters and memory.
    pub fn to_html(&self) -> String {
        let mut o = String::from("<!doctype html><meta charset=utf-8><title>AuraStudio diagnostic</title><style>body{font:14px system-ui;margin:24px;max-width:1100px}table{border-collapse:collapse;margin:8px 0 20px}td,th{padding:3px 10px;border-bottom:1px solid #ddd;text-align:right}td:first-child,th:first-child{text-align:left}.s{color:#b00}</style><h1>AuraStudio diagnostic</h1><table>");
        for (k, v) in &self.header { let _ = write!(o, "<tr><td>{}</td><td>{}</td></tr>", he(k), he(v)); }
        o.push_str("</table><h2>Suspects</h2>");
        if self.suspects.is_empty() { o.push_str("<p>None.</p>"); }
        for s in &self.suspects { let _ = write!(o, "<p class=s><b>{}</b> ({}) — {}<br><small>{}</small></p>", he(s.rule), he(&s.scenario), he(&s.what), he(s.hint)); }
        for r in &self.runs {
            let m = &r.snap.memory;
            let _ = write!(o, "<h2>{} — run {}</h2><p>{} recorded; private bytes {}, working set {}, heap {} (peak {}), {} allocations ({})</p>",
                he(&r.scenario), r.run, ms(r.snap.span), om(m.private_bytes), om(m.working_set), om(m.heap), om(m.heap_peak), m.allocs, mb(m.alloc_bytes));
            let table = |o: &mut String, title: &str, zones: Vec<&crate::record::ZoneStat>| {
                let _ = write!(o, "<h3>{}</h3><table><tr><th>zone</th><th>thread</th><th>calls</th><th>self</th><th>total</th><th>p50</th><th>p95</th><th>p99</th><th>max</th><th>allocs</th><th>bytes</th></tr>", he(title));
                for z in zones.into_iter().take(10) {
                    let _ = write!(o, "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                        he(&z.zone), he(&z.thread), z.calls, ms(z.self_t), ms(z.total), ms(z.p50), ms(z.p95), ms(z.p99), ms(z.max), z.allocs, mb(z.alloc_bytes));
                }
                o.push_str("</table>");
            };
            let mut by = r.snap.zones.iter().collect::<Vec<_>>();
            table(&mut o, "Top zones by self time", by.clone());
            by.sort_by_key(|z| std::cmp::Reverse(z.p99));
            table(&mut o, "Top zones by p99", by.clone());
            by.sort_by_key(|z| std::cmp::Reverse(z.alloc_bytes));
            table(&mut o, "Top allocation sites (bytes)", by);
            if !r.snap.counters.is_empty() {
                o.push_str("<h3>Counters</h3><table>");
                for (k, v) in &r.snap.counters { let _ = write!(o, "<tr><td>{}</td><td>{v}</td></tr>", he(k)); }
                o.push_str("</table>");
            }
        }
        o
    }
}

fn ms(ns: u64) -> String {
    if ns >= 1_000_000 { format!("{:.2} ms", ns as f64 / 1e6) } else { format!("{:.1} µs", ns as f64 / 1e3) }
}

fn mb(b: u64) -> String { format!("{:.1} MB", b as f64 / (1024.0 * 1024.0)) }

fn om(b: Option<u64>) -> String { b.map_or_else(|| "n/a".into(), mb) }

fn opt(v: Option<u64>) -> String { v.map_or_else(|| "null".into(), |v| v.to_string()) }

fn num(v: f64) -> String { if v.is_finite() { format!("{v}") } else { "null".into() } }

/// A JSON string.
fn js(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => { let _ = write!(o, "\\u{:04x}", c as u32); }
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// HTML text.
fn he(s: &str) -> String { s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;") }

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alloc::MemoryNow;
    use crate::record::ZoneStat;

    fn snap(private: u64, p99: u64) -> Snapshot {
        Snapshot {
            span: 1_000_000_000,
            zones: vec![ZoneStat { zone: "merge \"x\"".into(), thread: "main".into(), calls: 10, total: 5, self_t: 5, max: p99, p50: 1, p95: 2, p99, allocs: 50, alloc_bytes: 4096 }],
            counters: vec![("tiles".into(), 3)],
            samples: vec![("queue".into(), 2, 1.0, 0.0, 1.0, 0.5)],
            memory: MemoryNow { private_bytes: Some(private), ..Default::default() },
        }
    }

    #[test]
    fn rules_find_slow_zones_allocations_and_drift() {
        let mut r = Report::default();
        r.add_run("paint", snap(100 << 20, 1_000));
        r.add_run("paint", snap(130 << 20, 40_000_000));
        r.check(&Rules { no_alloc: vec!["merge \"x\""], ..Rules::default() });
        let rules: Vec<_> = r.suspects.iter().map(|s| s.rule).collect();
        assert_eq!(rules.iter().filter(|x| **x == "slow zone").count(), 1);
        assert_eq!(rules.iter().filter(|x| **x == "allocates").count(), 2);
        assert!(rules.contains(&"drift"));
        assert_eq!(r.runs[1].run, 2);
    }

    #[test]
    fn json_is_escaped_and_html_has_the_tables() {
        let mut r = Report { header: vec![("gpu".into(), "Arc <140T>".into())], ..Default::default() };
        r.add_run("idle", snap(1, 1));
        let j = r.to_json();
        assert!(j.contains("\"merge \\\"x\\\"\""));
        assert!(j.contains("\"private_bytes\": 1"));
        assert_eq!(j.matches('{').count(), j.matches('}').count());
        let h = r.to_html();
        assert!(h.contains("Arc &lt;140T&gt;") && h.contains("Top zones by self time"));
    }
}
