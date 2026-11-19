//! Picks up the session a remote caller asked makit to type into once it comes back up after an `update install`
//! restart (#D-2122): 总管's own `/loop` uses this to reconnect to itself without hardcoding any session name or
//! relying on it always being the foreground tab.
//!
//! One-shot: the continuation is a small file next to `link.sock`, removed the moment it is read, so a later
//! restart (a crash, a manual restart) never replays it.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui::{App, Entity};
use makit_core::link::{InputKind, WriteStep};

use crate::state::AppState;
use crate::workspace::WorkspaceView;

#[derive(Clone, Debug, PartialEq)]
pub struct Continuation {
    pub session_id: String,
    pub text: String,
}

fn path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".makit").join("after_restart.json"))
}

/// Written right before `update install` restarts. Best-effort: losing this means 总管 has to type it again, not
/// a crash, so a write failure here is silently swallowed the same way `makit_core::last_running::write` is.
pub fn stash(c: &Continuation) {
    let Some(p) = path() else { return };
    let v = serde_json::json!({"session_id": c.session_id, "text": c.text});
    let _ = std::fs::write(p, v.to_string());
}

fn take() -> Option<Continuation> {
    let p = path()?;
    let data = std::fs::read_to_string(&p).ok()?;
    let _ = std::fs::remove_file(&p);
    let v: serde_json::Value = serde_json::from_str(&data).ok()?;
    Some(Continuation { session_id: v.get("session_id")?.as_str()?.to_string(), text: v.get("text")?.as_str()?.to_string() })
}

/// How often to check whether the session has settled.
const POLL_EVERY: Duration = Duration::from_secs(2);
/// Give up after this long: a session that never goes idle (stuck, or its id no longer exists after the update)
/// must not poll forever.
const GIVE_UP_AFTER: Duration = Duration::from_secs(600);

/// Called once at startup, right after the window exists. A no-op on every ordinary launch (nothing stashed).
pub fn resume_if_pending(handle: gpui::WindowHandle<crate::app::Root>, cx: &mut App) {
    let Some(c) = take() else { return };
    let Ok(ws): Result<Entity<WorkspaceView>, _> = handle.update(cx, |root, _, _| root.workspace_view()) else { return };
    cx.spawn(async move |cx| {
        // #D-2149/#D-2150: this used to only *wait* for the session to go idle, on the assumption that someone
        // else would call `resume` on it -- but when the stashed session is the one orchestrating the upgrade
        // in the first place (总管's own), nothing is left alive after the old process quits to ever make that
        // call, so the wait below always ran out the clock. Resume it here instead: `allow_new_tab: true`
        // because a fresh restart never has an existing tab for anything yet.
        let result = cx.update(|cx| crate::overlays::session_ops::resume_session(&c.session_id, true, cx));
        match result {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => log::warn!(target: "update", "after-restart continuation for {}: resume failed: {}", c.session_id, e.message),
            Err(_) => return, // 窗口已经不在了
        }
        let deadline = Instant::now() + GIVE_UP_AFTER;
        loop {
            let idle = cx.update(|cx| AppState::global(cx).read(cx).session(&c.session_id).is_some_and(|m| m.running && m.status == "idle")).unwrap_or(false);
            if idle {
                break;
            }
            if Instant::now() >= deadline {
                log::warn!(target: "update", "after-restart continuation for {}: gave up waiting for it to go idle", c.session_id);
                return;
            }
            cx.background_executor().timer(POLL_EVERY).await;
        }
        let Ok(steps) = makit_core::link::plan_input("idle", "", &InputKind::Text(c.text.clone())) else { return };
        for step in steps {
            match step {
                WriteStep::Bytes(bytes) => {
                    let sid = c.session_id.clone();
                    let _ = cx.update(|cx| {
                        let _ = ws.update(cx, |w, cx| w.write_to_session(&sid, bytes, cx));
                    });
                }
                WriteStep::Wait(d) => cx.background_executor().timer(d).await,
            }
        }
    })
    .detach();
}
