//! 通知（E 包）：hook 服务、去重、在屏判定、系统通知、Dock 角标、通知中心。
//!
//! F0 只占了位置：`actions::notify::ToggleNotificationCenter`（⌘I）已进快捷键表；
//! 通知记录和三个开关已在 `NativeState.notifications` / `NativeState.notify`（从 localStorage 导入过来）。
//!
//! ⚠️ hook 服务（`makit_core::hook::start`）F0 **故意没起**：它绑定 `~/.claude/makit/hook.sock` 前会先删掉
//! 旧 socket —— 和正在跑的 Tauri 版同时开就会抢走它的 hook。接入时要先定「两个版本同时开谁收 hook」。
