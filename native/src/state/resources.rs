//! Reads the two live numbers [`super::capacity::capacity_limit`] checks against (#D-2066/#D-2078): macOS only for
//! now (see the TRD's risk note on Windows). Kept thin on purpose -- the parsing that can actually go wrong is a
//! pure function tested against real `vm_stat` output; the two platform calls around it are trivial enough that
//! real-machine behavior is the only meaningful test (same tradeoff the codex `pgrep` binding already makes).

/// Parses `vm_stat`'s page counts into an *available*-memory ratio (#D-2118: the capacity cap used to count only
/// `Pages free`, which on a machine with a lot of reclaimable cache reads as "almost out" while the system is
/// nowhere near it -- a real report had 7.5% free rejecting every resume while 78% was actually available). Pure
/// so this is testable without a real `vm_stat` call. `total` is approximated as free + active + inactive +
/// speculative + wired + compressed, which covers every page vm_stat accounts for as "real" memory (excluding
/// derived counters like page-in/page-out); `available` is free + inactive + speculative + purgeable -- the pages
/// the kernel can hand to a new process without swapping, same reckoning as `memory_pressure`.
pub fn parse_vm_stat_available_ratio(output: &str) -> Option<f32> {
    let mut pages: std::collections::HashMap<&str, u64> = std::collections::HashMap::new();
    for line in output.lines() {
        let Some((label, rest)) = line.split_once(':') else { continue };
        let label = label.trim();
        // Not every line is a page count (the header line has no trailing number at all): skip anything that
        // does not parse instead of failing the whole read over one line we do not care about.
        let Ok(n) = rest.trim().trim_end_matches('.').parse::<u64>() else { continue };
        pages.insert(label, n);
    }
    let get = |label: &str| pages.get(label).copied().unwrap_or(0);
    let free = get("Pages free");
    let inactive = get("Pages inactive");
    let speculative = get("Pages speculative");
    let total = free + get("Pages active") + inactive + speculative + get("Pages wired down") + get("Pages occupied by compressor");
    if total == 0 {
        return None;
    }
    let available = free + inactive + speculative + get("Pages purgeable");
    Some((available as f32 / total as f32).min(1.0))
}

/// `None` when `vm_stat` is missing or its output does not parse (platform other than macOS, or the format
/// changed). Callers that gate on this should fail open (treat "could not tell" as "do not block"): refusing to
/// open a session everywhere this cannot be read would make the cap itself the outage.
pub fn available_mem_ratio() -> Option<f32> {
    let output = std::process::Command::new("vm_stat").output().ok()?;
    if !output.status.success() {
        return None;
    }
    parse_vm_stat_available_ratio(&String::from_utf8_lossy(&output.stdout))
}

/// 1-minute load average via the C library call (no subprocess, works the same on macOS and Linux). `libc` is a
/// unix-only dependency (see `native/Cargo.toml`), and Windows has no equivalent "load average" concept at all
/// (TRD #316 risk note); `None` there is the correct, permanent answer, not a gap to close later.
#[cfg(unix)]
pub fn load_1m() -> Option<f32> {
    let mut loads = [0.0f64; 3];
    let n = unsafe { libc::getloadavg(loads.as_mut_ptr(), 3) };
    if n < 1 {
        return None;
    }
    Some(loads[0] as f32)
}

#[cfg(not(unix))]
pub fn load_1m() -> Option<f32> {
    None
}

/// What the makit process itself asks the OS for its file-descriptor soft limit at startup (#D-2090): tonight's
/// incident (0.2.13 open with 299 fds against the usual macOS default of 256) came from `RLIMIT_NOFILE`, not from
/// anything makit's own code could have freed -- link accept, archive reads and scan-cache writes all failed the
/// same way once the process ran out. Generous on purpose: a too-high soft limit costs nothing by itself, while
/// hitting the real ceiling again reproduces tonight across several unrelated subsystems at once.
pub const FD_LIMIT_TARGET: u64 = 8192;

/// A fd soft limit that is at least `target`, never above the hard limit, and never *lower* than what the process
/// already has (raising only ever goes up). Pure so the clamping logic is testable without a real `getrlimit`.
pub fn next_fd_limit(soft: u64, hard: u64, target: u64) -> u64 {
    target.min(hard).max(soft)
}

/// Raises `RLIMIT_NOFILE`'s soft limit toward [`FD_LIMIT_TARGET`] (clamped to the hard limit) if it is not there
/// already. `None` means the read/write itself failed (should not happen on a sane unix system, but this must not
/// be the thing that stops the app from starting); `Some((old, new))` is `(old, old)` when nothing needed to change.
#[cfg(unix)]
pub fn raise_fd_limit() -> Option<(u64, u64)> {
    let mut lim = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) } != 0 {
        return None;
    }
    let new = next_fd_limit(lim.rlim_cur, lim.rlim_max, FD_LIMIT_TARGET);
    if new == lim.rlim_cur {
        return Some((lim.rlim_cur, lim.rlim_cur));
    }
    let old = lim.rlim_cur;
    lim.rlim_cur = new;
    if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &lim) } != 0 {
        return None;
    }
    Some((old, new))
}

#[cfg(not(unix))]
pub fn raise_fd_limit() -> Option<(u64, u64)> {
    None
}

/// The process's own current fd soft limit, for the monitor below to judge "how close" against. Separate call
/// from `raise_fd_limit` (which already knows this) because the monitor runs on its own timer, long after startup.
#[cfg(unix)]
pub fn fd_soft_limit() -> Option<u64> {
    let mut lim = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) } != 0 {
        return None;
    }
    Some(lim.rlim_cur)
}

#[cfg(not(unix))]
pub fn fd_soft_limit() -> Option<u64> {
    None
}

/// How many fds this process currently has open. `/dev/fd` is a live view of the calling process's own descriptor
/// table on both macOS and Linux -- cheap (one `readdir`, no `/proc` parsing) and needs no new dependency.
#[cfg(unix)]
pub fn open_fd_count() -> Option<usize> {
    std::fs::read_dir("/dev/fd").ok().map(|d| d.count())
}

#[cfg(not(unix))]
pub fn open_fd_count() -> Option<usize> {
    None
}

/// Fraction of the soft limit already in use at which the monitor should warn (#D-2090): high enough to not be
/// noise from a session or two opening a burst of files, low enough to leave time to act before hitting the wall
/// tonight's incident hit at 100%.
pub const FD_WARN_RATIO: f32 = 0.8;

/// Whether `count` open fds against `soft_limit` warrants a warning log. `soft_limit == 0` (could not read it)
/// never warns -- same fail-open stance as the capacity cap itself: a reading that cannot be taken is not a reason
/// to raise an alarm that cannot be acted on.
pub fn fd_pressure(count: usize, soft_limit: u64) -> bool {
    soft_limit > 0 && count as f64 >= soft_limit as f64 * FD_WARN_RATIO as f64
}

/// Starts the periodic fd-count log (#D-2090): every [`MONITOR_INTERVAL`], log the current count at `info`, and at
/// `warn` once it crosses [`FD_WARN_RATIO`] of the soft limit -- the only sign of tonight's incident that showed up
/// *before* things started failing was the fd count climbing, so this is what the next one should be caught by.
pub const MONITOR_INTERVAL: std::time::Duration = std::time::Duration::from_secs(300);

pub fn start_fd_monitor(cx: &mut gpui::App) {
    cx.spawn(async move |cx| loop {
        cx.background_executor().timer(MONITOR_INTERVAL).await;
        let Some(count) = open_fd_count() else { continue };
        match fd_soft_limit() {
            Some(limit) if fd_pressure(count, limit) => {
                log::warn!(target: "resources", "open fds: {count} of a {limit} soft limit ({}%) -- approaching the ceiling that caused #317", (count as f64 / limit as f64 * 100.0) as u32);
            }
            Some(limit) => log::info!(target: "resources", "open fds: {count} of a {limit} soft limit"),
            None => log::info!(target: "resources", "open fds: {count} (soft limit unknown)"),
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "Mach Virtual Memory Statistics: (page size of 4096 bytes)\n\
Pages free:                                   114256.\n\
Pages active:                                3314366.\n\
Pages inactive:                              3266230.\n\
Pages speculative:                             30452.\n\
Pages throttled:                                   0.\n\
Pages wired down:                            1122718.\n\
Pages purgeable:                               10691.\n\
\"Translation faults\":                     5725740400.\n\
Pages copy-on-write:                       277277300.\n\
Pages zero filled:                        3749052339.\n\
Pages reactivated:                          27039633.\n\
Pages purged:                                5903958.\n\
File-backed pages:                           3375108.\n\
Anonymous pages:                             3235940.\n\
Pages stored in compressor:                  2424354.\n\
Pages occupied by compressor:                 540151.\n\
Decompressions:                             14603062.\n\
Compressions:                               22254718.\n\
Pageins:                                   100721271.\n\
Pageouts:                                     627597.\n\
Swapins:                                       39579.\n\
Swapouts:                                     391537.\n";

    #[test]
    fn parses_real_vm_stat_output() {
        let ratio = parse_vm_stat_available_ratio(SAMPLE).expect("parses");
        let total = 114256 + 3314366 + 3266230 + 30452 + 1122718 + 540151;
        let available = 114256 + 3266230 + 30452 + 10691;
        assert!((ratio - available as f32 / total as f32).abs() < 1e-6);
    }

    // #D-2118: 用户实测的真实事故 -- 只有 7.5% 是严格意义的 free，但 inactive/speculative/purgeable 合计让
    // 真正可用的有 78%，旧的只认 free 的算法会把这台机器误判成内存紧张，拒掉所有 resume
    #[test]
    fn low_free_but_high_available_reads_as_available_not_low_memory() {
        let sample = "Pages free:                                   75000.\n\
Pages active:                                200000.\n\
Pages inactive:                              600000.\n\
Pages speculative:                            50000.\n\
Pages wired down:                             50000.\n\
Pages purgeable:                              55000.\n\
Pages occupied by compressor:                 25000.\n";
        let raw_free: f32 = 75000.0 / 1_000_000.0;
        let ratio = parse_vm_stat_available_ratio(sample).expect("parses");
        assert!((raw_free - 0.075).abs() < 1e-6, "sanity: this sample's raw free ratio is ~7.5%");
        assert!((ratio - 0.78).abs() < 1e-6, "available should read ~78%, not the ~7.5% raw free");
    }

    #[test]
    fn garbage_input_does_not_panic_and_reports_unknown() {
        assert_eq!(parse_vm_stat_available_ratio(""), None);
        assert_eq!(parse_vm_stat_available_ratio("not vm_stat output at all"), None);
        assert_eq!(parse_vm_stat_available_ratio("Pages free: not-a-number."), None);
    }

    #[cfg(unix)]
    #[test]
    fn load_1m_returns_a_plausible_number_on_this_real_machine() {
        // Not a platform-behavior claim beyond "the call works and returns something sane" -- the actual
        // threshold logic is tested in `capacity.rs` with synthetic numbers.
        let l = load_1m().expect("getloadavg works on macOS/Linux");
        assert!(l >= 0.0 && l < 10_000.0);
    }

    #[cfg(not(unix))]
    #[test]
    fn load_1m_is_unknown_on_non_unix() {
        assert_eq!(load_1m(), None);
    }

    #[test]
    fn next_fd_limit_only_ever_raises_and_never_past_the_hard_limit() {
        assert_eq!(next_fd_limit(256, 10_240, FD_LIMIT_TARGET), FD_LIMIT_TARGET);
        assert_eq!(next_fd_limit(256, 4_096, FD_LIMIT_TARGET), 4_096, "clamped by a lower hard limit");
        assert_eq!(next_fd_limit(10_000, 10_240, FD_LIMIT_TARGET), 10_000, "already above target: never lowered");
    }

    #[test]
    fn fd_pressure_warns_only_past_the_ratio_and_never_with_an_unknown_limit() {
        assert!(!fd_pressure(100, 256), "well under the ratio");
        assert!(fd_pressure(205, 256), "205/256 ≈ 80%");
        assert!(!fd_pressure(204, 256), "just under 80%");
        assert!(!fd_pressure(1_000_000, 0), "limit could not be read: fail open, do not warn on garbage");
    }

    #[cfg(unix)]
    #[test]
    fn open_fd_count_is_plausible_on_this_real_machine() {
        // Every test process has stdio plus whatever the test harness itself opened: never zero.
        let n = open_fd_count().expect("/dev/fd is readable on macOS/Linux");
        assert!(n > 0 && n < 10_000);
    }

    #[cfg(not(unix))]
    #[test]
    fn open_fd_count_is_unknown_on_non_unix() {
        assert_eq!(open_fd_count(), None);
    }
}
