//! 通知（E 包，#215 方案）：hook 服务、两路来源去重、在屏判定、系统通知、Dock 角标、⌘I 通知中心、铃铛、窗口闪一下。
//!
//! | 文件 | 内容 |
//! |---|---|
//! | `model` | 四类通知 `Kind`、信号 `Signal`（纯数据） |
//! | `classify` | hook 事件 / 状态文件变化 → 信号；两路主从 `Arbiter`（纯函数 + 测试） |
//! | `deliver` | 投递决策 + 在屏判定（纯函数 + 测试） |
//! | `book` | 通知记录 reducer：每会话一条、已读、撤回、未读数、恢复不补发（纯逻辑 + 测试） |
//! | `center` | `Notifier` 实体：把上面串起来，起 hook 服务，管抽屉状态 |
//! | `system` | UNUserNotificationCenter（横幅 / 点击 / 撤回 / 授权）+ Dock 角标 |
//! | `ui` | 抽屉、铃铛、窗口闪一下 |
//! | `selftest` | `MAKIT_NATIVE_SELFTEST=notify` 无人值守自检 |
//!
//! ## hook 归属（用户 2026-09-27 拍板）
//!
//! Tauri 版和 GPUI 版同时开时，**谁后启动 hook 就给谁**，和现在的行为一致：`makit_core::hook::start`
//! 绑定 `~/.claude/makit/hook.sock` 前先删旧 socket。先启动的那个还挂在被 unlink 的旧 socket 上，
//! 不报错、只是再也收不到 hook，退回状态文件兜底（等待照样有通知）。见 `center::start_hook_server`。
//! 测试实例用假 HOME（socket 在假目录里，不抢），或设 `MAKIT_NATIVE_NO_HOOK=1` 干脆不起。
//!
//! ## 给别的包的接口
//!
//! - C 工作区：标题栏里 `.child(crate::notify::bell(cx))`；订阅 `NotifyEvent::FlashPane` 在那个 pane 上闪落点牌
//! - B 侧栏：订阅 `NotifyEvent::RevealSession` 在侧栏里定位（只定位，不打开）
//! - A 终端：用户在某个会话的终端里打字时 `Notifier::global(cx).update(cx, |n, cx| n.mark_session_read(sid, cx))`
//! - D 设置：`n.read(cx).permission`（`Permission::label()`）、`request_permission()`、`refresh_permission()`、
//!   `send_test(cx)`；开关在 `prefs.notify`（system / approval / user / completed / sound）

pub mod book;
pub mod center;
pub mod classify;
pub mod deliver;
pub mod model;
#[cfg(unix)]
pub mod selftest;
pub mod system;
pub mod ui;

use gpui::{App, WindowHandle};

pub use center::{Notifier, NotifyEvent};
pub use system::Permission;
pub use ui::{bell, layer};

/// 开窗口之后调：跟踪窗口焦点（在屏判定要用；窗口回到前台时把眼前那个会话标已读）
pub fn attach_window(handle: WindowHandle<crate::app::Root>, cx: &mut App) {
    let Some(n) = Notifier::try_global(cx) else { return };
    let _ = handle.update(cx, |_, window, cx| {
        let n2 = n.clone();
        cx.observe_window_activation(window, move |_, window, cx| {
            let active = window.is_window_active();
            n2.update(cx, |x, cx| x.set_window_active(active, cx));
        })
        .detach();
        let active = window.is_window_active();
        n.update(cx, |x, cx| x.set_window_active(active, cx));
    });
}
