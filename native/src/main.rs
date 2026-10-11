//! makit 的 GPUI 原生界面（#226）。入口只调 `app::run`，结构见 lib.rs。

// Release builds on Windows are GUI-subsystem so no console window opens next to the app.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

fn main() {
    makit_native::app::run();
}
