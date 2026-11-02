//! Reused tile buffers: the pixels a tile gets when first painted and the
//! stroke masks come from here and go back here, so a long session does
//! not allocate and free the same few kilobytes thousands of times
//! (Krita's tile info pool). Buffers not reused for a while are freed
//! (`trim`), and the pool never holds more than `MAX_BYTES`.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use aurastudio_core::Pixel;

/// The most the pool keeps.
pub const MAX_BYTES: usize = 64 << 20;

struct Pool {
    /// Per pixel type: a `Vec<(Vec<P>, Instant)>` (boxed once per type).
    pixels: HashMap<TypeId, Box<dyn Any + Send>>,
    masks:  Vec<(Box<[u8]>, Instant)>,
    bytes:  usize,
}

static POOL: Mutex<Option<Pool>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut Pool) -> R) -> R {
    let mut g = POOL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let p = g.get_or_insert_with(|| Pool { pixels: HashMap::new(), masks: Vec::new(), bytes: 0 });
    f(p)
}

/// `n` pixels of `fill` (reused when one of that size is waiting).
pub fn take_pixels<P: Pixel>(n: usize, fill: P) -> Vec<P> {
    let got = with(|p| {
        let list = p.pixels.get_mut(&TypeId::of::<P>())?.downcast_mut::<Vec<(Vec<P>, Instant)>>()?;
        let i = list.iter().rposition(|(v, _)| v.len() == n)?;
        let (v, _) = list.swap_remove(i);
        p.bytes -= n * P::BYTES;
        Some(v)
    });
    match got {
        Some(mut v) => { v.fill(fill); v }
        None => vec![fill; n],
    }
}

/// Give back a buffer no longer used.
pub fn give_pixels<P: Pixel>(v: Vec<P>) {
    let b = v.len() * P::BYTES;
    if b == 0 { return; }
    with(|p| {
        if p.bytes + b > MAX_BYTES { return; }
        let list = p.pixels.entry(TypeId::of::<P>()).or_insert_with(|| Box::new(Vec::<(Vec<P>, Instant)>::new()));
        if let Some(list) = list.downcast_mut::<Vec<(Vec<P>, Instant)>>() {
            list.push((v, Instant::now()));
            p.bytes += b;
        }
    });
}

/// `n` zero bytes (a stroke mask).
pub fn take_mask(n: usize) -> Box<[u8]> {
    let got = with(|p| {
        let i = p.masks.iter().rposition(|(m, _)| m.len() == n)?;
        let (m, _) = p.masks.swap_remove(i);
        p.bytes -= n;
        Some(m)
    });
    match got {
        Some(mut m) => { m.fill(0); m }
        None => vec![0u8; n].into_boxed_slice(),
    }
}

pub fn give_mask(m: Box<[u8]>) {
    let n = m.len();
    with(|p| {
        if p.bytes + n > MAX_BYTES { return; }
        p.masks.push((m, Instant::now()));
        p.bytes += n;
    });
}

/// Free what has waited longer than `idle` (call it when the app is
/// idle; `Duration::ZERO` empties the pool).
pub fn trim(idle: Duration) {
    with(|p| {
        let now = Instant::now();
        let mut freed = 0;
        p.masks.retain(|(m, t)| { let keep = now.duration_since(*t) < idle; if !keep { freed += m.len(); } keep });
        p.bytes -= freed;
        let mut freed = 0;
        for list in p.pixels.values_mut() {
            freed += trim_any(list.as_mut(), now, idle);
        }
        p.bytes -= freed;
    });
}

/// (Each pixel type's list is trimmed through its own type.)
fn trim_any(list: &mut (dyn Any + Send), now: Instant, idle: Duration) -> usize {
    fn go<P: Pixel>(l: &mut Vec<(Vec<P>, Instant)>, now: Instant, idle: Duration) -> usize {
        let mut freed = 0;
        l.retain(|(v, t)| { let keep = now.duration_since(*t) < idle; if !keep { freed += v.len() * P::BYTES; } keep });
        freed
    }
    use aurastudio_core::{Rgba16, Rgba8, RgbaF16, RgbaF32};
    if let Some(l) = list.downcast_mut::<Vec<(Vec<Rgba8>, Instant)>>() { return go(l, now, idle); }
    if let Some(l) = list.downcast_mut::<Vec<(Vec<u8>, Instant)>>() { return go(l, now, idle); }
    if let Some(l) = list.downcast_mut::<Vec<(Vec<Rgba16>, Instant)>>() { return go(l, now, idle); }
    if let Some(l) = list.downcast_mut::<Vec<(Vec<RgbaF16>, Instant)>>() { return go(l, now, idle); }
    if let Some(l) = list.downcast_mut::<Vec<(Vec<RgbaF32>, Instant)>>() { return go(l, now, idle); }
    0
}

/// Bytes waiting in the pool (Memory report).
pub fn held_bytes() -> usize { with(|p| p.bytes) }
