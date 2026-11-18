//! Reads the two live numbers [`super::capacity::capacity_limit`] checks against (#D-2066/#D-2078): macOS only for
//! now (see the TRD's risk note on Windows). Kept thin on purpose -- the parsing that can actually go wrong is a
//! pure function tested against real `vm_stat` output; the two platform calls around it are trivial enough that
//! real-machine behavior is the only meaningful test (same tradeoff the codex `pgrep` binding already makes).

/// Parses `vm_stat`'s page counts into a free-memory ratio (#D-2066). Pure so this is testable without a real
/// `vm_stat` call; `total` is approximated as free + active + inactive + speculative + wired + compressed, which
/// covers every page vm_stat accounts for as "real" memory (excluding derived counters like page-in/page-out).
pub fn parse_vm_stat_free_ratio(output: &str) -> Option<f32> {
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
    let total = free + get("Pages active") + get("Pages inactive") + get("Pages speculative") + get("Pages wired down") + get("Pages occupied by compressor");
    if total == 0 {
        return None;
    }
    Some(free as f32 / total as f32)
}

/// `None` when `vm_stat` is missing or its output does not parse (platform other than macOS, or the format
/// changed). Callers that gate on this should fail open (treat "could not tell" as "do not block"): refusing to
/// open a session everywhere this cannot be read would make the cap itself the outage.
pub fn free_mem_ratio() -> Option<f32> {
    let output = std::process::Command::new("vm_stat").output().ok()?;
    if !output.status.success() {
        return None;
    }
    parse_vm_stat_free_ratio(&String::from_utf8_lossy(&output.stdout))
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
        let ratio = parse_vm_stat_free_ratio(SAMPLE).expect("parses");
        let total = 114256 + 3314366 + 3266230 + 30452 + 1122718 + 540151;
        assert!((ratio - 114256.0 / total as f32).abs() < 1e-6);
        assert!(ratio < 0.02, "this sample is the night it almost ran out: should read as very low free");
    }

    #[test]
    fn garbage_input_does_not_panic_and_reports_unknown() {
        assert_eq!(parse_vm_stat_free_ratio(""), None);
        assert_eq!(parse_vm_stat_free_ratio("not vm_stat output at all"), None);
        assert_eq!(parse_vm_stat_free_ratio("Pages free: not-a-number."), None);
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
}
