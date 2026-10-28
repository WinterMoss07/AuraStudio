//! A crash keeps the work: the document shown is written to the recovery
//! file (offered at the next start, as after a power cut) and a message
//! says so, instead of the window just vanishing.
//! A crash report (version, graphics, panic message, backtrace) is written
//! next to it, so the cause survives a closed console.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError, TryLockError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use aurastudio_document::SharedDoc;

static CURRENT: Mutex<Option<SharedDoc>> = Mutex::new(None);
/// Once per run: a panic while rescuing is not rescued again.
static STARTED: AtomicBool = AtomicBool::new(false);
static RECOVERY: Mutex<Option<PathBuf>> = Mutex::new(None);
/// Where crash reports go.
static REPORTS: Mutex<Option<PathBuf>> = Mutex::new(None);
/// The report written for this run's first panic.
static REPORT: Mutex<Option<PathBuf>> = Mutex::new(None);
/// Facts about this run, copied into a report (graphics, pen backend...).
static NOTES: Mutex<Vec<(&'static str, String)>> = Mutex::new(Vec::new());

/// Reports kept: the oldest go past this.
const KEEP_REPORTS: usize = 20;

/// The document to save if the app crashes.
pub fn watch(doc: &SharedDoc) {
    *CURRENT.lock().unwrap_or_else(PoisonError::into_inner) = Some(std::sync::Arc::clone(doc));
}

/// A fact about this run for the crash report (set again: replaced).
pub fn note(key: &'static str, value: impl Into<String>) {
    let mut notes = NOTES.lock().unwrap_or_else(PoisonError::into_inner);
    let value = value.into();
    match notes.iter_mut().find(|(k, _)| *k == key) {
        Some(n) => n.1 = value,
        None => notes.push((key, value)),
    }
}

/// Install the panic hook; `recovery` is the file the next start offers,
/// `reports` the folder for crash reports.
pub fn install(recovery: Option<PathBuf>, reports: Option<PathBuf>) {
    *RECOVERY.lock().unwrap_or_else(PoisonError::into_inner) = recovery;
    *REPORTS.lock().unwrap_or_else(PoisonError::into_inner) = reports;
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default(info);
        if STARTED.load(Ordering::SeqCst) { return; }
        write_report(info);
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

/// The first panic of the run, written to a file of its own.
fn write_report(info: &std::panic::PanicHookInfo) {
    let mut report = REPORT.lock().unwrap_or_else(PoisonError::into_inner);
    if report.is_some() { return; }
    let Some(dir) = REPORTS.lock().unwrap_or_else(PoisonError::into_inner).clone() else { return };
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let text = report_text(info, now, &NOTES.lock().unwrap_or_else(PoisonError::into_inner));
    let path = dir.join(format!("crash-{}.txt", stamp(now, '_', '-')));
    if std::fs::create_dir_all(&dir).is_err() { return; }
    match std::fs::write(&path, text) {
        Ok(()) => {
            log::error!("crash report: {}", path.display());
            prune(&dir, KEEP_REPORTS);
            *report = Some(path);
        }
        Err(e) => log::error!("crash report not written: {e}"),
    }
}

fn report_text(info: &std::panic::PanicHookInfo, now: u64, notes: &[(&'static str, String)]) -> String {
    use std::fmt::Write;
    let message = info.payload().downcast_ref::<&str>().map(|s| s.to_string())
        .or_else(|| info.payload().downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "(no message)".into());
    let place = info.location().map_or("(unknown)".into(), |l| format!("{}:{}:{}", l.file(), l.line(), l.column()));
    let thread = std::thread::current();
    let mut t = String::new();
    let _ = writeln!(t, "AuraStudio {} crash report", env!("CARGO_PKG_VERSION"));
    let _ = writeln!(t, "time:     {} UTC", stamp(now, ' ', ':'));
    let _ = writeln!(t, "system:   {} {}", std::env::consts::OS, std::env::consts::ARCH);
    for (k, v) in notes { let _ = writeln!(t, "{k:<9} {v}", k = format!("{k}:")); }
    let _ = writeln!(t, "thread:   {}", thread.name().unwrap_or("(unnamed)"));
    let _ = writeln!(t, "where:    {place}");
    let _ = writeln!(t, "message:  {message}");
    let _ = writeln!(t, "\nbacktrace:\n{}", std::backtrace::Backtrace::force_capture());
    t
}

/// "2026-10-28 14:03:59" from seconds since 1970 (UTC), with `mid`
/// between the date and the time and `sep` between hours, minutes, seconds.
fn stamp(secs: u64, mid: char, sep: char) -> String {
    let days = (secs / 86_400) as i64;
    let s = secs % 86_400;
    // Days to a civil date (H. Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}{mid}{:02}{sep}{:02}{sep}{:02}", s / 3600, s / 60 % 60, s % 60)
}

/// Keep the `keep` newest reports of `dir` (their names sort by time).
fn prune(dir: &Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut reports: Vec<PathBuf> = entries.flatten().map(|e| e.path())
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("crash-") && n.ends_with(".txt")))
        .collect();
    reports.sort();
    let extra = reports.len().saturating_sub(keep);
    for old in &reports[..extra] { let _ = std::fs::remove_file(old); }
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
    let mut text = String::from("AuraStudio ran into a problem and has to close.");
    if saved { text.push_str("\n\nYour unsaved work was kept: it will be offered the next time AuraStudio starts."); }
    if let Some(p) = REPORT.lock().unwrap_or_else(PoisonError::into_inner).as_ref() {
        text.push_str(&format!("\n\nA crash report was written to:\n{}", p.display()));
    }
    rfd::MessageDialog::new().set_title("AuraStudio").set_description(text).set_level(rfd::MessageLevel::Error).show();
    std::process::exit(1);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamps_are_civil_utc() {
        assert_eq!(stamp(0, ' ', ':'), "1970-01-01 00:00:00");
        // 2000-02-29 (leap day) 12:34:56.
        assert_eq!(stamp(951_827_696, ' ', ':'), "2000-02-29 12:34:56");
        assert_eq!(stamp(1_793_196_239, '_', '-'), "2026-10-28_14-03-59");
    }

    #[test]
    fn prune_keeps_the_newest_reports() {
        let dir = std::env::temp_dir().join(format!("aurs-crash-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for d in 1..=5 { std::fs::write(dir.join(format!("crash-2026-10-0{d}_00-00-00.txt")), "x").unwrap(); }
        std::fs::write(dir.join("notes.txt"), "kept").unwrap();
        prune(&dir, 2);
        let mut left: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().into_string().unwrap()).collect();
        left.sort();
        assert_eq!(left, ["crash-2026-10-04_00-00-00.txt", "crash-2026-10-05_00-00-00.txt", "notes.txt"]);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
