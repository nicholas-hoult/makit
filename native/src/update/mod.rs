//! In-app update (#200): background check, titlebar pill, "update and restart".
//!
//! Flow: `start` checks shortly after launch and then about once a day (`makit_core::update`); a newer,
//! not-skipped release makes `available` return it and the titlebar shows a pill. `install` downloads the
//! release `.dmg`, verifies its SHA-256 against the release API digest, stages the new bundle next to the old
//! one, then a detached helper swaps the bundles after this process exits and relaunches (see `apply`).
//!
//! Anything that cannot be done safely (brew-managed install, dev instance, not running from an `.app`,
//! unwritable install dir, no installer/digest in the release) degrades to "open the release page".

pub mod apply;

use std::time::Duration;

use gpui::{App, Global};
use makit_core::update::{self as core_update, ReleaseInfo};

use crate::state::AppState;
use crate::{tr, ts};

/// Background check interval
const CHECK_EVERY_SECS: u64 = 24 * 3600;
/// First check this long after launch, so it never competes with startup
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(20);

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Idle,
    Checking,
    Available(ReleaseInfo),
    /// Download / verify / stage in progress for this release
    Installing(ReleaseInfo),
}

struct Model(Status);
impl Global for Model {}

pub fn status(cx: &App) -> Status {
    cx.try_global::<Model>().map(|m| m.0.clone()).unwrap_or(Status::Idle)
}

fn set(status: Status, cx: &mut App) {
    cx.set_global(Model(status));
    cx.refresh_windows();
}

/// The release to advertise (newer than the running version and not skipped), if any
pub fn available(cx: &App) -> Option<ReleaseInfo> {
    match status(cx) {
        Status::Available(r) | Status::Installing(r) => Some(r),
        _ => None,
    }
}

pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Starts the periodic check (called once at startup). Dev instances and selftests never check
pub fn start(cx: &mut App) {
    // Debug: pretend this release is available (no installer, so "Update" opens the page); lets the UI be looked at
    if let Some(v) = std::env::var("MAKIT_FAKE_UPDATE").ok().filter(|v| !v.trim().is_empty()) {
        let release = ReleaseInfo {
            page_url: format!("https://github.com/nicholas-hoult/makit/releases/tag/v{v}"),
            version: v,
            notes: String::new(),
            asset: None,
        };
        set(Status::Available(release), cx);
        return;
    }
    if crate::app::is_dev_instance() || std::env::var_os("MAKIT_NATIVE_SELFTEST").is_some() || std::env::var_os("MAKIT_NO_UPDATE_CHECK").is_some() {
        return;
    }
    cx.spawn(async move |cx| {
        cx.background_executor().timer(FIRST_CHECK_DELAY).await;
        loop {
            let due = cx
                .update(|cx| core_update::check_due(AppState::global(cx).read(cx).prefs.update.last_check, now_secs(), CHECK_EVERY_SECS))
                .unwrap_or(false);
            if due {
                let _ = cx.update(|cx| check_now(false, cx));
            }
            cx.background_executor().timer(Duration::from_secs(3600)).await;
        }
    })
    .detach();
}

/// Checks now. `manual` = the user asked, so the outcome is reported with a toast
pub fn check_now(manual: bool, cx: &mut App) {
    if matches!(status(cx), Status::Checking | Status::Installing(_)) {
        return;
    }
    let previous = status(cx);
    set(Status::Checking, cx);
    cx.spawn(async move |cx| {
        let result = cx.background_executor().spawn(async { core_update::fetch_latest() }).await;
        let _ = cx.update(|cx| {
            let skipped = AppState::global(cx).read(cx).prefs.update.skipped.clone();
            AppState::global(cx).update(cx, |s, cx| s.update_prefs(cx, |p| p.update.last_check = now_secs()));
            match result {
                Ok(release) if core_update::is_available(current_version(), &release.version, if manual { "" } else { &skipped }) => {
                    set(Status::Available(release), cx);
                }
                Ok(_) => {
                    set(Status::Idle, cx);
                    if manual {
                        crate::overlays::show_toast(ts!("update.toast.up_to_date", version = current_version()), 2500, cx);
                    }
                }
                Err(e) => {
                    log::info!(target: "update", "update check failed: {e}");
                    set(if matches!(previous, Status::Checking) { Status::Idle } else { previous }, cx);
                    if manual {
                        crate::overlays::show_toast(tr!("update.toast.check_failed"), 3000, cx);
                    }
                }
            }
        });
    })
    .detach();
}

/// "Skip this version": the pill disappears until a newer release appears
pub fn skip(version: &str, cx: &mut App) {
    let v = version.to_string();
    AppState::global(cx).update(cx, |s, cx| s.update_prefs(cx, |p| p.update.skipped = v));
    set(Status::Idle, cx);
}

/// Update and restart; falls back to opening the release page whenever replacing the app is not safe
pub fn install(release: ReleaseInfo, cx: &mut App) {
    if matches!(status(cx), Status::Installing(_)) {
        return;
    }
    match apply::plan(&release, crate::app::is_dev_instance()) {
        apply::Plan::Brew { command } => {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(command.clone()));
            crate::overlays::show_toast(ts!("update.toast.brew_copied", cmd = command), 5000, cx);
        }
        apply::Plan::OpenPage => {
            cx.open_url(&release.page_url);
            crate::overlays::show_toast(tr!("update.toast.opened_page"), 2500, cx);
        }
        apply::Plan::InPlace(job) => {
            set(Status::Installing(release.clone()), cx);
            crate::overlays::show_toast(ts!("update.toast.downloading", version = release.version.clone()), 4000, cx);
            cx.spawn(async move |cx| {
                let result = cx.background_executor().spawn(async move { apply::stage_and_launch(&job) }).await;
                let _ = cx.update(|cx| match result {
                    Ok(()) => {
                        crate::overlays::show_toast(tr!("update.toast.installing"), 3000, cx);
                        // The helper waits for this process to exit, then swaps the bundle and relaunches
                        cx.spawn(async move |cx| {
                            cx.background_executor().timer(Duration::from_millis(600)).await;
                            let _ = cx.update(|cx| cx.quit());
                        })
                        .detach();
                    }
                    Err(e) => {
                        log::warn!(target: "update", "in-place update failed: {e}");
                        set(Status::Available(release), cx);
                        crate::overlays::show_toast(ts!("update.toast.install_failed", reason = e), 6000, cx);
                    }
                });
            })
            .detach();
        }
    }
}
