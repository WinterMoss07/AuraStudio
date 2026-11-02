//! What the system says about this process.

/// (working set, private bytes) of this process, as the system counts.
#[cfg(windows)]
pub fn process_memory() -> (Option<u64>, Option<u64>) {
    use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX};
    use windows::Win32::System::Threading::GetCurrentProcess;
    let mut c = PROCESS_MEMORY_COUNTERS_EX { cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32, ..Default::default() };
    // SAFETY: `c` is a PROCESS_MEMORY_COUNTERS_EX (it starts with a
    // PROCESS_MEMORY_COUNTERS) and `cb` says how big it is.
    let ok = unsafe { GetProcessMemoryInfo(GetCurrentProcess(), (&mut c as *mut PROCESS_MEMORY_COUNTERS_EX).cast::<PROCESS_MEMORY_COUNTERS>(), c.cb) };
    if ok.is_ok() { (Some(c.WorkingSetSize as u64), Some(c.PrivateUsage as u64)) } else { (None, None) }
}

/// (resident set, private bytes ≈ anonymous resident) from /proc.
#[cfg(target_os = "linux")]
pub fn process_memory() -> (Option<u64>, Option<u64>) {
    let Ok(s) = std::fs::read_to_string("/proc/self/status") else { return (None, None) };
    let kb = |key: &str| s.lines().find(|l| l.starts_with(key)).and_then(|l| l.split_whitespace().nth(1)).and_then(|v| v.parse::<u64>().ok()).map(|v| v * 1024);
    (kb("VmRSS:"), kb("RssAnon:"))
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn process_memory() -> (Option<u64>, Option<u64>) { (None, None) }
