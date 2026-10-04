//! Performance instrumentation (#218): timings of key operations, available in the packaged build too.
//!
//! - Clock: zero is the process start (the moment recorded by the kernel). The frontend takes `process_start` from `startup_info()` (Tauri command `perf_startup`)
//!   and converts its own timestamps onto the same clock.
//! - Persistence: `~/.claude/makit/perf.log`, JSON Lines, one event per line; rotated to `perf.log.1` beyond 2MB
//!   (only one old file is kept).
//! - Log writes go through a background thread (commands are all async), so the main thread is not used.

use serde_json::Value;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;

fn epoch_ms(t: SystemTime) -> f64 {
    t.duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64() * 1000.0).unwrap_or(0.0)
}

pub fn now_epoch_ms() -> f64 {
    epoch_ms(SystemTime::now())
}

/// The moment the process started (epoch milliseconds). On macOS it is the kernel-recorded value, so load time before main counts too;
/// on other platforms it falls back to the first call.
pub fn process_start_epoch_ms() -> f64 {
    static START: std::sync::LazyLock<f64> = std::sync::LazyLock::new(|| {
        #[cfg(target_os = "macos")]
        {
            let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
            let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
            let got = unsafe {
                libc::proc_pidinfo(
                    std::process::id() as libc::c_int,
                    libc::PROC_PIDTBSDINFO,
                    0,
                    &mut info as *mut _ as *mut std::ffi::c_void,
                    size,
                )
            };
            if got == size {
                return info.pbi_start_tvsec as f64 * 1000.0 + info.pbi_start_tvusec as f64 / 1000.0;
            }
        }
        now_epoch_ms()
    });
    *START
}

/// Startup phases on the Rust side (run begins, window created, page loaded...), which wait for the frontend to fetch them and merge them with its own phases into one startup record
static STARTUP_MARKS: Mutex<Vec<(String, f64)>> = Mutex::new(Vec::new());

pub fn startup_mark(stage: &str) {
    process_start_epoch_ms(); // make sure the zero point is fixed before the first measurement
    if let Ok(mut marks) = STARTUP_MARKS.lock() {
        marks.push((stage.to_string(), now_epoch_ms()));
    }
}

#[derive(serde::Serialize, Debug, Clone)]
pub struct StartupInfo {
    pub process_start: f64,
    pub marks: Vec<(String, f64)>,
}

/// Process start time + the Rust-side startup phases recorded so far (this is what Tauri's `perf_startup` command returns)
pub fn startup_info() -> StartupInfo {
    StartupInfo {
        process_start: process_start_epoch_ms(),
        marks: STARTUP_MARKS.lock().map(|m| m.clone()).unwrap_or_default(),
    }
}

fn log_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude").join("makit").join("perf.log"))
}

/// Append some lines; if the file is over the limit beforehand, rotate first (the old one becomes `.1`, overwriting the even older one)
fn append_lines_to(path: &Path, lines: &[Value], max_bytes: u64) -> std::io::Result<()> {
    static WRITE: Mutex<()> = Mutex::new(());
    let _guard = WRITE.lock();
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    if fs::metadata(path).map(|m| m.len() >= max_bytes).unwrap_or(false) {
        fs::rename(path, path.with_extension("log.1"))?;
    }
    let mut buf = Vec::new();
    for line in lines {
        serde_json::to_writer(&mut buf, line)?;
        buf.push(b'\n');
    }
    fs::OpenOptions::new().create(true).append(true).open(path)?.write_all(&buf)
}

/// Record one event. Adds a timestamp (milliseconds relative to process start); a failed write only logs and does not affect functionality.
pub fn record(mut event: Value) {
    if let Value::Object(map) = &mut event {
        map.entry("t").or_insert_with(|| Value::from((now_epoch_ms() - process_start_epoch_ms()).round()));
    }
    if let Some(p) = log_path() {
        if let Err(e) = append_lines_to(&p, &[event], MAX_LOG_BYTES) {
            log::warn!(target: "perf", "writing perf.log failed: {e}");
        }
    }
}

/// Write a batch of events to disk as-is (no timestamp added). The Tauri frontend accumulates a batch and sends it (at most once per second), which goes through here
pub fn record_batch(events: Vec<Value>) {
    if events.is_empty() {
        return;
    }
    if let Some(p) = log_path() {
        if let Err(e) = append_lines_to(&p, &events, MAX_LOG_BYTES) {
            log::warn!(target: "perf", "writing perf.log failed: {e}");
        }
    }
}

/// In the packaged build this file is the only way to investigate "why is it slow", so the format and rotation must be pinned down:
/// one JSON per line (`tail -f` / `jq` can read it directly), and beyond the limit only one old file is kept, so it never grows without bound.
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("makit-perf-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir.join("perf.log")
    }

    #[test]
    fn appends_one_json_per_line() {
        let p = tmp("lines");
        append_lines_to(&p, &[json!({"kind": "a", "ms": 1}), json!({"kind": "b"})], 1 << 20).unwrap();
        append_lines_to(&p, &[json!({"kind": "c"})], 1 << 20).unwrap();
        let text = fs::read_to_string(&p).unwrap();
        let kinds: Vec<String> = text
            .lines()
            .map(|l| serde_json::from_str::<Value>(l).unwrap()["kind"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(kinds, ["a", "b", "c"]);
    }

    #[test]
    fn rotates_when_over_limit_keeping_one_old_file() {
        let p = tmp("rotate");
        let big = json!({"pad": "x".repeat(100)});
        append_lines_to(&p, &[big.clone()], 50).unwrap(); // first time: the file does not exist, write directly
        append_lines_to(&p, &[json!({"n": 2})], 50).unwrap(); // already over 50 bytes -> rotate first
        assert!(fs::read_to_string(p.with_extension("log.1")).unwrap().contains("pad"));
        assert_eq!(fs::read_to_string(&p).unwrap().trim(), r#"{"n":2}"#);
        append_lines_to(&p, &[big], 5).unwrap(); // now 8 bytes, over 5 -> rotate again: the old .1 is overwritten, only one is kept
        assert_eq!(fs::read_to_string(p.with_extension("log.1")).unwrap().trim(), r#"{"n":2}"#);
    }

    #[test]
    fn process_start_is_before_now_and_recent() {
        let start = process_start_epoch_ms();
        let now = now_epoch_ms();
        assert!(start <= now, "零点不能在现在之后");
        assert!(now - start < 10.0 * 60.0 * 1000.0, "测试进程不会跑了 10 分钟以上：零点取错了");
    }
}
