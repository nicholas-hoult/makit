//! 性能埋点（#221）：和 Tauri 版写同一个 `~/.claude/makit/perf.log`（复用 `makit_core::perf`），
//! 多一个 `"app": "gpui"` 字段区分。启动时间线以进程启动（内核记录的时刻）为 0，
//! 和 Tauri 版的 `startup` 记录同一把尺子。

use std::sync::Mutex;

/// 写一条 perf.log。统一补上 `build`（debug / release）：debug 版的帧耗时比 release 高一个量级，
/// 混在一个日志里分不出来就没法判断「卡」到底是代码问题还是没开优化
pub fn record(mut v: serde_json::Value) {
    if let Some(o) = v.as_object_mut() {
        o.entry("build").or_insert(serde_json::json!(if cfg!(debug_assertions) { "debug" } else { "release" }));
    }
    makit_core::perf::record(v);
}
use makit_core::perf::{now_epoch_ms, process_start_epoch_ms};

static MARKS: Mutex<Vec<(String, f64)>> = Mutex::new(Vec::new());

pub fn mark(stage: &str) {
    process_start_epoch_ms();
    if let Ok(mut m) = MARKS.lock() {
        m.push((stage.to_string(), now_epoch_ms()));
    }
    if std::env::var_os("MAKIT_TIMING").is_some() {
        eprintln!("[startup] {stage} {:.0}ms", now_epoch_ms() - process_start_epoch_ms());
    }
}

/// 侧栏第一次带着数据画出来之后调用：写一条 startup
pub fn report_startup(sessions: usize) {
    let start = process_start_epoch_ms();
    let marks = MARKS.lock().map(|m| m.clone()).unwrap_or_default();
    let stages: Vec<serde_json::Value> = marks
        .iter()
        .map(|(k, t)| serde_json::json!([k, (t - start).round()]))
        .collect();
    let ms = marks.last().map(|(_, t)| (t - start).round()).unwrap_or(0.0);
    record(serde_json::json!({
        "kind": "startup",
        "app": "gpui",
        "ms": ms,
        "sessions": sessions,
        "stages": stages,
    }));
}
