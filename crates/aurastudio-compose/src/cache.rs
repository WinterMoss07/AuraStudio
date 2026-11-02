//! A cache of merged pieces by content key, bounded in bytes (the oldest
//! use goes first). Shared between the cores of one merge.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use aurastudio_core::blend::Premul;

struct Entry { px: Arc<Vec<Premul>>, used: u64 }

#[derive(Default)]
struct Inner { map: HashMap<u64, Entry>, bytes: usize, clock: u64 }

/// Isolated folders' content, flattened, by key (a folder of 20 layers
/// merged once while what is around it changes).
pub struct FolderCache {
    inner:  Mutex<Inner>,
    budget: usize,
}

impl Default for FolderCache {
    fn default() -> Self { Self::new(64 << 20) }
}

impl FolderCache {
    pub fn new(budget: usize) -> Self { Self { inner: Mutex::new(Inner::default()), budget } }

    pub(crate) fn get(&self, key: u64) -> Option<Arc<Vec<Premul>>> {
        let mut g = self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        g.clock += 1;
        let now = g.clock;
        let e = g.map.get_mut(&key)?;
        e.used = now;
        aurastudio_core::count!("folder cache hits", 1);
        Some(Arc::clone(&e.px))
    }

    pub(crate) fn put(&self, key: u64, px: Arc<Vec<Premul>>) {
        let size = px.len() * 16;
        if size > self.budget / 4 { return; }
        let mut g = self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        g.clock += 1;
        let now = g.clock;
        if let Some(old) = g.map.insert(key, Entry { px, used: now }) { g.bytes -= old.px.len() * 16; }
        g.bytes += size;
        while g.bytes > self.budget {
            let Some((&k, _)) = g.map.iter().min_by_key(|(_, e)| e.used) else { break };
            if let Some(e) = g.map.remove(&k) { g.bytes -= e.px.len() * 16; }
        }
    }

    /// Bytes held.
    pub fn bytes(&self) -> usize { self.inner.lock().map_or(0, |g| g.bytes) }

    pub fn clear(&self) { if let Ok(mut g) = self.inner.lock() { *g = Inner::default(); } }
}
