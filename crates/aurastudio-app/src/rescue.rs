//! A crash keeps the work: the document shown is written to the recovery
//! file (offered at the next start, as after a power cut) and a message
//! says so, instead of the window just vanishing.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError, TryLockError};
use std::time::{Duration, Instant};

use aurastudio_document::SharedDoc;

static CURRENT: Mutex<Option<SharedDoc>> = Mutex::new(None);
/// Once per run: a panic while rescuing is not rescued again.
static STARTED: AtomicBool = AtomicBool::new(false);
static RECOVERY: Mutex<Option<PathBuf>> = Mutex::new(None);

/// The document to save if the app crashes.
pub fn watch(doc: &SharedDoc) {
    *CURRENT.lock().unwrap_or_else(PoisonError::into_inner) = Some(std::sync::Arc::clone(doc));
}

/// Install the panic hook; `recovery` is the file the next start offers.
pub fn install(recovery: Option<PathBuf>) {
    *RECOVERY.lock().unwrap_or_else(PoisonError::into_inner) = recovery;
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default(info);
        if STARTED.load(Ordering::SeqCst) { return; }
        // The document free (or left poisoned): save it now. Held by the
        // thread that panicked: after unwinding (`after_unwind`).
        if let Some(saved) = try_save(Duration::from_millis(300)) { finish(saved); }
    }));
}

/// A thread that caught its own panic: the document is free now.
pub fn after_unwind() {
    if STARTED.load(Ordering::SeqCst) { return; }
    let saved = try_save(Duration::from_secs(2)).unwrap_or(false);
    finish(saved);
}

/// Some(saved?) once done, None if the document stayed locked.
fn try_save(wait: Duration) -> Option<bool> {
    let doc = CURRENT.lock().unwrap_or_else(PoisonError::into_inner).clone();
    let Some(doc) = doc else { return Some(false) };
    let path = RECOVERY.lock().unwrap_or_else(PoisonError::into_inner).clone();
    let t = Instant::now();
    let guard = loop {
        match doc.try_lock() {
            Ok(g) => break g,
            Err(TryLockError::Poisoned(p)) => break p.into_inner(),
            Err(TryLockError::WouldBlock) if t.elapsed() < wait => std::thread::sleep(Duration::from_millis(10)),
            Err(TryLockError::WouldBlock) => return None,
        }
    };
    if STARTED.swap(true, Ordering::SeqCst) { return Some(false); }
    // Nothing unsaved: the file on disk has it all.
    if !guard.is_modified() { return Some(false); }
    let Some(path) = path else { return Some(false) };
    let snap = aurastudio_file::snapshot(&guard, guard.path.as_deref());
    drop(guard);
    if let Some(dir) = path.parent() { let _ = std::fs::create_dir_all(dir); }
    match aurastudio_file::save(&snap, &path) {
        Ok(()) => { log::error!("crash: work saved to {}", path.display()); Some(true) }
        Err(e) => { log::error!("crash: couldn't save the work: {e}"); Some(false) }
    }
}

fn finish(saved: bool) -> ! {
    STARTED.store(true, Ordering::SeqCst);
    let text = if saved {
        "AuraStudio ran into a problem and has to close.\n\nYour unsaved work was kept: it will be offered the next time AuraStudio starts."
    } else {
        "AuraStudio ran into a problem and has to close."
    };
    rfd::MessageDialog::new().set_title("AuraStudio").set_description(text).set_level(rfd::MessageLevel::Error).show();
    std::process::exit(1);
}
