//! The recorder behind `core::diag`: zones (calls, total and self time,
//! percentiles), counters and samples, kept per thread and merged when a
//! snapshot is taken. Nothing is allocated per mark once a zone and a
//! thread have been seen.

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use aurastudio_core::diag::Hooks;

/// Most zones told apart (zone 0 is "outside any zone").
pub const MAX_ZONES: usize = 1024;

/// Buckets of the duration histogram: 4 per power of two of nanoseconds,
/// up to 2^48 ns (3 days).
const BUCKETS: usize = 4 * 48;

fn start() -> Instant { *START.get_or_init(Instant::now) }
static START: OnceLock<Instant> = OnceLock::new();

/// Nanoseconds since the recorder started.
pub fn now_ns() -> u64 { start().elapsed().as_nanos() as u64 }

/// Zone names by index (`ZONES[0]` = outside any zone).
static ZONES: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

/// The index of `name` (made the first time; the same literal at two
/// places shares one).
fn zone_index(name: &'static str) -> u16 {
    let mut z = ZONES.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if z.is_empty() { z.push("(outside zones)"); }
    if let Some(i) = z.iter().position(|n| *n == name) { return i as u16; }
    if z.len() >= MAX_ZONES { return 0; }
    z.push(name);
    (z.len() - 1) as u16
}

pub(crate) fn zone_name(i: usize) -> &'static str {
    ZONES.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(i).copied().unwrap_or("?")
}

thread_local! {
    /// The zone this thread is in now (for the allocation counter: a
    /// const cell, it never allocates).
    pub(crate) static CURRENT: Cell<u16> = const { Cell::new(0) };
    static DATA: Arc<Mutex<ThreadData>> = register();
}

#[derive(Clone, Copy)]
struct Frame { zone: u16, start: u64, children: u64, parent: u16 }

#[derive(Clone)]
pub(crate) struct ZoneAcc {
    pub calls:  u64,
    pub total:  u64,
    pub self_t: u64,
    pub max:    u64,
    pub hist:   [u32; BUCKETS],
}

impl Default for ZoneAcc {
    fn default() -> Self { Self { calls: 0, total: 0, self_t: 0, max: 0, hist: [0; BUCKETS] } }
}

fn bucket(ns: u64) -> usize {
    if ns < 8 { return ns as usize; }
    let p = 63 - ns.leading_zeros() as usize; // 2^p <= ns
    let sub = ((ns >> (p - 2)) & 3) as usize;
    (p * 4 + sub).min(BUCKETS - 1)
}

/// The lower edge of a bucket, in ns.
fn bucket_floor(b: usize) -> u64 {
    // (Buckets 8..12 are never used: 8 ns starts at bucket 12.)
    if b < 12 { return (b as u64).min(8); }
    let (p, sub) = (b / 4, b % 4);
    (1u64 << p) + ((sub as u64) << (p - 2))
}

impl ZoneAcc {
    /// The duration under which `q` (0..1) of the calls fall (bucket
    /// precision: about ±12 %).
    pub fn quantile(&self, q: f64) -> u64 {
        let target = (self.calls as f64 * q).ceil().max(1.0) as u64;
        let mut seen = 0u64;
        for (b, n) in self.hist.iter().enumerate() {
            seen += *n as u64;
            if seen >= target { return bucket_floor(b + 1).min(self.max); }
        }
        self.max
    }
}

pub(crate) struct ThreadData {
    pub name:     String,
    stack:        Vec<Frame>,
    pub zones:    Vec<ZoneAcc>,
    pub counters: HashMap<&'static str, i64>,
    pub samples:  HashMap<&'static str, Series>,
}

/// The values of one sample: count, last, min, max, sum (for the mean).
#[derive(Clone, Copy, Default)]
pub struct Series { pub n: u64, pub last: f64, pub min: f64, pub max: f64, pub sum: f64 }

static THREADS: Mutex<Vec<Arc<Mutex<ThreadData>>>> = Mutex::new(Vec::new());

fn register() -> Arc<Mutex<ThreadData>> {
    let name = std::thread::current().name().unwrap_or("unnamed").to_owned();
    let d = Arc::new(Mutex::new(ThreadData { name, stack: Vec::with_capacity(32), zones: Vec::new(), counters: HashMap::new(), samples: HashMap::new() }));
    THREADS.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(Arc::clone(&d));
    d
}

fn with_data(f: impl FnOnce(&mut ThreadData)) {
    // (`try_with`: nothing is recorded while a thread is being torn down.)
    let _ = DATA.try_with(|d| f(&mut d.lock().unwrap_or_else(std::sync::PoisonError::into_inner)));
}

#[cfg(feature = "tracy")]
thread_local! {
    /// The Tracy spans open on this thread (closed in reverse order).
    static SPANS: std::cell::RefCell<Vec<tracy_client::Span>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn begin(name: &'static str) -> u64 {
    #[cfg(feature = "tracy")]
    if let Some(c) = tracy_client::Client::running() {
        let span = c.span_alloc(Some(name), "", "", 0, 0);
        let _ = SPANS.try_with(|s| s.borrow_mut().push(span));
    }
    let zone = zone_index(name);
    let t = now_ns();
    let parent = CURRENT.with(|c| c.replace(zone));
    with_data(|d| d.stack.push(Frame { zone, start: t, children: 0, parent }));
    t
}

fn end(_name: &'static str, _token: u64) {
    let t = now_ns();
    #[cfg(feature = "tracy")]
    let _ = SPANS.try_with(|s| s.borrow_mut().pop());
    with_data(|d| {
        let Some(f) = d.stack.pop() else { return };
        let dur = t.saturating_sub(f.start);
        if let Some(p) = d.stack.last_mut() { p.children += dur; }
        let i = f.zone as usize;
        if d.zones.len() <= i { d.zones.resize(i + 1, ZoneAcc::default()); }
        let z = &mut d.zones[i];
        z.calls += 1;
        z.total += dur;
        z.self_t += dur.saturating_sub(f.children);
        z.max = z.max.max(dur);
        z.hist[bucket(dur)] += 1;
        CURRENT.with(|c| c.set(f.parent));
    });
}

fn count(name: &'static str, n: i64) { with_data(|d| *d.counters.entry(name).or_insert(0) += n); }

fn sample(name: &'static str, v: f64) {
    #[cfg(feature = "tracy")]
    if let Some(c) = tracy_client::Client::running() {
        // (Tracy plots need a static name: the zone registry keeps them.)
        if let Some(p) = plot_name(name) { c.plot(p, v); }
    }
    with_data(|d| {
        let s = d.samples.entry(name).or_insert(Series { n: 0, last: v, min: v, max: v, sum: 0.0 });
        s.n += 1;
        s.last = v;
        s.min = s.min.min(v);
        s.max = s.max.max(v);
        s.sum += v;
    });
}

#[cfg(feature = "tracy")]
fn plot_name(name: &'static str) -> Option<tracy_client::PlotName> {
    static PLOTS: Mutex<Vec<(&'static str, tracy_client::PlotName)>> = Mutex::new(Vec::new());
    let mut p = PLOTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((_, n)) = p.iter().find(|(k, _)| *k == name) { return Some(*n); }
    let n = tracy_client::PlotName::new_leak(name.to_owned());
    p.push((name, n));
    Some(n)
}

static HOOKS: Hooks = Hooks { begin, end, count, sample };

/// Start recording the marks of every crate (once, at start).
pub fn install() {
    let _ = start();
    aurastudio_core::diag::set_hooks(&HOOKS);
}

/// One zone of one thread in a snapshot (times in ns).
#[derive(Clone, Debug)]
pub struct ZoneStat {
    pub zone:   String,
    pub thread: String,
    pub calls:  u64,
    pub total:  u64,
    pub self_t: u64,
    pub max:    u64,
    pub p50:    u64,
    pub p95:    u64,
    pub p99:    u64,
    /// Allocations made while this zone was the innermost (all threads).
    pub allocs: u64,
    pub alloc_bytes: u64,
}

/// What was recorded since the start or the last `reset`.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    /// Length of the recording, ns.
    pub span:     u64,
    pub zones:    Vec<ZoneStat>,
    pub counters: Vec<(String, i64)>,
    /// name, count, last, min, max, mean
    pub samples:  Vec<(String, u64, f64, f64, f64, f64)>,
    pub memory:   crate::alloc::MemoryNow,
}

static RESET_AT: Mutex<u64> = Mutex::new(0);

/// Everything recorded so far, merged across threads.
pub fn snapshot() -> Snapshot {
    let threads = THREADS.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
    let allocs = crate::alloc::per_zone();
    let mut s = Snapshot { span: now_ns().saturating_sub(*RESET_AT.lock().unwrap_or_else(std::sync::PoisonError::into_inner)), ..Default::default() };
    let mut counters: HashMap<&'static str, i64> = HashMap::new();
    let mut samples: HashMap<&'static str, Series> = HashMap::new();
    for t in threads {
        let d = t.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        for (i, z) in d.zones.iter().enumerate() {
            if z.calls == 0 { continue; }
            let (n, b) = allocs.get(i).copied().unwrap_or((0, 0));
            s.zones.push(ZoneStat { zone: zone_name(i).to_owned(), thread: d.name.clone(), calls: z.calls, total: z.total, self_t: z.self_t, max: z.max, p50: z.quantile(0.5), p95: z.quantile(0.95), p99: z.quantile(0.99), allocs: n, alloc_bytes: b });
        }
        for (k, v) in &d.counters { *counters.entry(k).or_insert(0) += v; }
        for (k, v) in &d.samples {
            let e = samples.entry(k).or_insert(Series { n: 0, last: v.last, min: v.min, max: v.max, sum: 0.0 });
            e.n += v.n; e.last = v.last; e.min = e.min.min(v.min); e.max = e.max.max(v.max); e.sum += v.sum;
        }
    }
    let mut c: Vec<_> = counters.into_iter().map(|(k, v)| (k.to_owned(), v)).collect();
    c.sort();
    s.counters = c;
    let mut sm: Vec<_> = samples.into_iter().map(|(k, v)| (k.to_owned(), v.n, v.last, v.min, v.max, if v.n > 0 { v.sum / v.n as f64 } else { 0.0 })).collect();
    sm.sort_by(|a, b| a.0.cmp(&b.0));
    s.samples = sm;
    s.zones.sort_by_key(|z| std::cmp::Reverse(z.self_t));
    s.memory = crate::alloc::memory_now();
    s
}

/// Forget what was recorded (between two runs of a scenario). Zones in
/// progress keep going.
pub fn reset() {
    for t in THREADS.lock().unwrap_or_else(std::sync::PoisonError::into_inner).iter() {
        let mut d = t.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        d.zones.clear();
        d.counters.clear();
        d.samples.clear();
    }
    crate::alloc::reset_counts();
    *RESET_AT.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = now_ns();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buckets_are_ordered_and_close() {
        let mut prev = 0;
        for b in 0..BUCKETS {
            let f = bucket_floor(b);
            assert!(f >= prev);
            prev = f;
        }
        for ns in (0..5000u64).chain([1 << 20, 123_456_789, 1 << 47]) {
            let b = bucket(ns);
            assert!(bucket_floor(b) <= ns && (b + 1 >= BUCKETS || ns < bucket_floor(b + 1).max(ns + 1)), "{ns}");
        }
        let mut z = ZoneAcc::default();
        for ns in [1000u64; 99].into_iter().chain([1_000_000]) {
            z.calls += 1; z.max = z.max.max(ns); z.hist[bucket(ns)] += 1;
        }
        let p50 = z.quantile(0.5);
        assert!((1000..=1250).contains(&p50), "{p50}");
        assert_eq!(z.quantile(1.0), 1_000_000);
    }

    #[test]
    fn nested_zones_split_self_time() {
        install();
        let worker = std::thread::Builder::new().name("nested-test".into()).spawn(|| {
            aurastudio_core::zone!("outer-test");
            std::thread::sleep(std::time::Duration::from_millis(2));
            {
                aurastudio_core::zone!("inner-test");
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            aurastudio_core::count!("things-test", 2);
        }).unwrap();
        worker.join().unwrap();
        let s = snapshot();
        let get = |n: &str| s.zones.iter().find(|z| z.zone == n && z.thread == "nested-test").cloned().unwrap();
        let (o, i) = (get("outer-test"), get("inner-test"));
        assert!(o.total >= i.total + 2_000_000);
        assert!(o.self_t < o.total - 4_000_000, "outer self time excludes inner");
        assert_eq!(i.calls, 1);
        assert!(s.counters.iter().any(|(k, v)| k == "things-test" && *v == 2));
    }
}
