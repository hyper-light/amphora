//! Amphora: the machine's memory, for the versioned storage's budget when the shell gives none
//! (G7 step 2, `AC-NS.2.1`; `docs/design/engine.md`, decision 13). The shell sets the budget from
//! the memory its process may use (`amphora::memory`, which reads the control groups on Linux) with
//! [`ConnectOpts::with_memory_budget`](crate::ConnectOpts::with_memory_budget); without it the
//! engine takes [`SHARE_DIVISOR`]th of the machine's memory.
//!
//! The system's interfaces here need `unsafe` (CLAUDE.md §12 names this file): `sysconf` on Unix,
//! `GlobalMemoryStatusEx` on Windows; elsewhere a typed refusal naming the platform.

use crate::error::LimboError;
use crate::Result;

/// The part of the machine's memory the storage takes by default: a quarter, as the shell's
/// process pool takes of the memory it may use (`amphora::db`, decision 26 of
/// `docs/design/store.md`): PostgreSQL's documentation advises a quarter of a dedicated server's
/// memory for its shared buffers ("Resource Consumption", `shared_buffers`), the cautious end of
/// what databases that size from memory take (CedarDB's buffer pool up to 45%, DuckDB 80%).
pub const SHARE_DIVISOR: u64 = 4;

/// The budget a storage takes when the shell gives none: [`SHARE_DIVISOR`]th of the machine's
/// memory.
pub fn default_budget() -> Result<u64> {
    Ok(machine_bytes()?.checked_div(SHARE_DIVISOR).unwrap_or(0))
}

/// The machine's physical memory in bytes: its pages times their size.
#[cfg(unix)]
#[expect(
    unsafe_code,
    reason = "sysconf is libc's; CLAUDE.md §12 names this file"
)]
pub fn machine_bytes() -> Result<u64> {
    // SAFETY: sysconf reads a system value by name; it takes no pointer and touches no memory of
    // ours. A name the system doesn't know returns -1, which the conversion below refuses.
    let (pages, size) = unsafe {
        (
            libc::sysconf(libc::_SC_PHYS_PAGES),
            libc::sysconf(libc::_SC_PAGESIZE),
        )
    };
    let pages = u64::try_from(pages).ok();
    let size = u64::try_from(size).ok();
    pages
        .zip(size)
        .and_then(|(p, s)| p.checked_mul(s))
        .filter(|&n| n > 0)
        .ok_or_else(|| {
            LimboError::InternalError(
                "versioned storage: the system doesn't say its memory (sysconf)".into(),
            )
        })
}

/// The machine's physical memory in bytes.
#[cfg(windows)]
#[expect(
    unsafe_code,
    reason = "GlobalMemoryStatusEx is Win32's; CLAUDE.md §12 names this file"
)]
pub fn machine_bytes() -> Result<u64> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut status = MEMORYSTATUSEX {
        dwLength: u32::try_from(std::mem::size_of::<MEMORYSTATUSEX>())
            .map_err(|_| LimboError::IntegerOverflow)?,
        dwMemoryLoad: 0,
        ullTotalPhys: 0,
        ullAvailPhys: 0,
        ullTotalPageFile: 0,
        ullAvailPageFile: 0,
        ullTotalVirtual: 0,
        ullAvailVirtual: 0,
        ullAvailExtendedVirtual: 0,
    };
    // SAFETY: `status` is a live MEMORYSTATUSEX whose dwLength is its size, as the call requires;
    // the call writes only within it and keeps no pointer to it.
    let ok = unsafe { GlobalMemoryStatusEx(&mut status) };
    if ok == 0 || status.ullTotalPhys == 0 {
        return Err(LimboError::InternalError(
            "versioned storage: the system doesn't say its memory (GlobalMemoryStatusEx)".into(),
        ));
    }
    Ok(status.ullTotalPhys)
}

/// Neither Unix nor Windows: the shell gives the budget.
#[cfg(not(any(unix, windows)))]
pub fn machine_bytes() -> Result<u64> {
    Err(LimboError::InvalidArgument(format!(
        "versioned storage: the machine's memory isn't known on {}; give a budget (ConnectOpts::with_memory_budget)",
        std::env::consts::OS
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// T-NS.2.10: ask the machine's memory and the default budget. Expect: some bytes, the budget a
    /// quarter of them.
    #[test]
    fn ns2_default_budget_is_a_share_of_the_machines_memory() {
        let machine = machine_bytes();
        assert!(matches!(machine, Ok(n) if n > 0));
        let budget = default_budget();
        assert_eq!(budget.ok(), machine.ok().map(|n| n / SHARE_DIVISOR));
    }
}
