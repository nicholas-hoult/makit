//! makit 的 GPUI 原生界面（#226）。入口只调 `app::run`，结构见 lib.rs。
//!
//! 唯一的例外是 `hook` 子命令（#215 第 1 期）：Claude Code 每次触发 hook 事件都会起一次这个进程，装在
//! `~/.claude/hooks/makit-hook.sh` 里调 `"$EXE" hook "$SOCK"`，所以要在建 GPUI 窗口之前就分流掉，
//! 不然每条 hook 事件都会闪一下界面。

// Release builds on Windows are GUI-subsystem so no console window opens next to the app.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

fn main() {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() == Some("hook") {
        std::process::exit(makit_core::hook::hook_cli(args.next().as_deref()));
    }
    makit_native::app::run();
}
