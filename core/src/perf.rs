//! 性能埋点（#218）：打包版也能看的关键操作耗时。
//!
//! - 时钟：以进程启动（内核记录的时刻）为 0。前端拿 `startup_info()`（Tauri 命令 `perf_startup`）里的 `process_start`
//!   把自己的时间戳换算到同一个时钟上。
//! - 落盘：`~/.claude/makit/perf.log`，JSON Lines，一行一个事件；超过 2MB 轮转成 `perf.log.1`
//!   （只留一份旧的）。
//! - 写日志走后台线程（命令都是 async），不占主线程。

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

/// 进程启动的时刻（epoch 毫秒）。macOS 取内核记录的，连 main 之前的加载时间也算进去；
/// 其他平台退回第一次调用的时刻。
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

/// Rust 侧的启动阶段（run 开始、窗口创建、页面加载……），等前端来取、和它自己的阶段合成一条启动记录
static STARTUP_MARKS: Mutex<Vec<(String, f64)>> = Mutex::new(Vec::new());

pub fn startup_mark(stage: &str) {
    process_start_epoch_ms(); // 确保零点在第一次打点之前就定下来
    if let Ok(mut marks) = STARTUP_MARKS.lock() {
        marks.push((stage.to_string(), now_epoch_ms()));
    }
}

#[derive(serde::Serialize, Debug, Clone)]
pub struct StartupInfo {
    pub process_start: f64,
    pub marks: Vec<(String, f64)>,
}

/// 进程启动时刻 + Rust 侧已打的启动阶段（Tauri 的 `perf_startup` 命令就是它）
pub fn startup_info() -> StartupInfo {
    StartupInfo {
        process_start: process_start_epoch_ms(),
        marks: STARTUP_MARKS.lock().map(|m| m.clone()).unwrap_or_default(),
    }
}

fn log_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude").join("makit").join("perf.log"))
}

/// 追加若干行；追加前超过上限就先轮转（旧的挪成 `.1`，覆盖更旧的那份）
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

/// 记一条事件。补上时间戳（相对进程启动的毫秒数），写失败只打日志、不影响功能。
pub fn record(mut event: Value) {
    if let Value::Object(map) = &mut event {
        map.entry("t").or_insert_with(|| Value::from((now_epoch_ms() - process_start_epoch_ms()).round()));
    }
    if let Some(p) = log_path() {
        if let Err(e) = append_lines_to(&p, &[event], MAX_LOG_BYTES) {
            log::warn!(target: "perf", "perf.log 写入失败: {e}");
        }
    }
}

/// 一批事件原样落盘（不补时间戳）。Tauri 前端攒一批再发（每秒最多一次），走的就是这里
pub fn record_batch(events: Vec<Value>) {
    if events.is_empty() {
        return;
    }
    if let Some(p) = log_path() {
        if let Err(e) = append_lines_to(&p, &events, MAX_LOG_BYTES) {
            log::warn!(target: "perf", "perf.log 写入失败: {e}");
        }
    }
}

/// 打包版查「为什么卡」只能靠这个文件，所以格式和轮转要钉住：
/// 一行一个 JSON（`tail -f` / `jq` 能直接读），超过上限只留一份旧的，不会无限长。
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
        append_lines_to(&p, &[big.clone()], 50).unwrap(); // 第一次：文件不存在，直接写
        append_lines_to(&p, &[json!({"n": 2})], 50).unwrap(); // 已超 50 字节 → 先轮转
        assert!(fs::read_to_string(p.with_extension("log.1")).unwrap().contains("pad"));
        assert_eq!(fs::read_to_string(&p).unwrap().trim(), r#"{"n":2}"#);
        append_lines_to(&p, &[big], 5).unwrap(); // 现在 8 字节，超 5 → 再轮转一次：旧的 .1 被覆盖，只留一份
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
