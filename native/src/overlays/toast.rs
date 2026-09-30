//! toast 时长（界面清单 N 节，照 App.tsx 各处的 setTimeout 数值）。单位毫秒。

/// 复制成功（「已复制 / 已复制路径 / 已复制命令」）
pub const COPY_OK: u64 = 1400;
/// 复制失败
pub const COPY_FAIL: u64 = 2000;
/// 已定位 session
pub const REVEALED: u64 = 1500;
/// 「该 session 正在运行中（PID x），不能重复启动」
pub const ALREADY_RUNNING: u64 = 3000;
/// 恢复启动目录成功（显示 detail）
pub const RECOVERED: u64 = 4000;
/// 恢复后面板需要关掉重开
pub const RECOVER_REOPEN: u64 = 5000;
/// 归档失败
pub const ARCHIVE_FAIL: u64 = 2000;
/// 在 Finder 中显示失败（Tauri 版静默吞掉；这里给一句，时长同其他失败）
pub const REVEAL_FAIL: u64 = 2000;
