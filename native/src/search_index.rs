//! Cross-session text search index for ⌘K's content-match group (#212, 总管 #D-2132).
//!
//! Built lazily on the first search, not at startup: scanning every session's transcript once (`open_transcript`
//! + a full `poll()` per session) is real work -- fine to pay once when someone actually searches content, wrong
//! to pay on every launch whether anyone ever searches or not. Runs on the background executor; the palette just
//! shows name matches only until it is ready, same degraded-but-correct state a fresh launch is already in.
//!
//! Only `search::Indexed` (a message's id, text, and its pre-lowered form) is kept per session, not the session's
//! full `Vec<Item>` (#D-2134): tool output, thinking and notices are the bulk of a real session's size and none
//! of them are searched, so there is no reason to hold them in memory indefinitely just because a few of that
//! session's messages were. Measured on a real ~3.7 GB / 217-session `~/.claude/projects`: the old full-`Item`
//! design's peak RSS was ~309 MB; this one's is ~149 MB.
//!
//! Stays current without a restart (#D-2134 增量更新): each session's entry also remembers the `mtime` it was
//! indexed at, and [`ensure_fresh`] only re-parses the sessions whose `mtime` has actually moved since.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use gpui::{App, Global};
use makit_core::search::{search_indexed, Hit, Indexed};
use makit_core::transcript::ItemKind;

/// Hard cap on how many sessions a single query reports a hit for (#D-2134③): past this, `search` stops
/// scanning further sessions rather than paying for a full pass just to produce rows nobody will scroll to.
pub const MAX_CONTENT_MATCHES: usize = 50;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BuildState {
    NotStarted,
    Building,
    Ready,
}

struct Corpus {
    state: BuildState,
    /// A background refresh (not the first build) is in flight; `state` stays `Ready` throughout so `search`
    /// keeps serving the previous results instead of going blank while it runs.
    refreshing: bool,
    /// session_id → (mtime indexed at, its searchable messages)
    by_session: HashMap<String, (i64, Vec<Indexed>)>,
}

struct Model(Arc<Mutex<Corpus>>);
impl Global for Model {}

fn model(cx: &mut App) -> Arc<Mutex<Corpus>> {
    if cx.try_global::<Model>().is_none() {
        cx.set_global(Model(Arc::new(Mutex::new(Corpus { state: BuildState::NotStarted, refreshing: false, by_session: HashMap::new() }))));
    }
    cx.global::<Model>().0.clone()
}

/// Whether the *first* build is still running (for the palette's "正在建索引" hint) -- a later incremental
/// refresh never reports this, since results from before it started are still valid and showing.
pub fn is_building(cx: &mut App) -> bool {
    model(cx).lock().unwrap().state == BuildState::Building
}

/// Parses one session's transcript down to its searchable messages (#D-2134: never materializes the full
/// `Vec<Item>` -- tool output / thinking / notices are dropped as they are read, not after).
fn index_one(session_id: &str) -> Option<Vec<Indexed>> {
    let mut reader = makit_core::sessions::open_transcript(session_id)?;
    reader.poll().ok()?;
    Some(
        (0..reader.len())
            .filter_map(|i| {
                let it = reader.item(i);
                match &it.kind {
                    ItemKind::User(t) | ItemKind::Assistant(t) => Some(Indexed::new(it.id.clone(), t.clone())),
                    _ => None,
                }
            })
            .collect(),
    )
}

/// Builds the index the first time this is called for real, and keeps it current after that: `sessions` is
/// every session's current `(id, mtime)`; a session missing from the index, or whose `mtime` moved since it was
/// indexed, gets (re)parsed in the background, everything else is left alone. Cheap to call on every keystroke
/// that has a non-empty query -- when nothing changed it is just a map comparison, and a build/refresh already
/// in flight makes this an immediate no-op.
pub fn ensure_fresh(sessions: Vec<(String, i64)>, cx: &mut App) {
    let m = model(cx);
    let to_index: Vec<String> = {
        let mut c = m.lock().unwrap();
        match c.state {
            BuildState::Building => return,
            BuildState::NotStarted => {
                c.state = BuildState::Building;
                sessions.iter().map(|(id, _)| id.clone()).collect()
            }
            BuildState::Ready => {
                if c.refreshing {
                    return;
                }
                let changed: Vec<String> = sessions.iter().filter(|(id, mtime)| c.by_session.get(id).map(|(m, _)| m) != Some(mtime)).map(|(id, _)| id.clone()).collect();
                if changed.is_empty() {
                    return;
                }
                c.refreshing = true;
                changed
            }
        }
    };
    let mtime_of: HashMap<String, i64> = sessions.into_iter().collect();
    cx.spawn(async move |cx| {
        let indexed = cx
            .background_executor()
            .spawn(async move { to_index.iter().filter_map(|id| index_one(id).map(|v| (id.clone(), v))).collect::<Vec<_>>() })
            .await;
        let _ = cx.update(|cx| {
            let m = model(cx);
            {
                let mut c = m.lock().unwrap();
                for (id, items) in indexed {
                    let mtime = mtime_of.get(&id).copied().unwrap_or(0);
                    c.by_session.insert(id, (mtime, items));
                }
                c.state = BuildState::Ready;
                c.refreshing = false;
            }
            // The palette only re-renders on its own query-change / state-change notifications; the index
            // finishing well after the keystroke that triggered it needs its own nudge to show up.
            cx.refresh_windows();
        });
    })
    .detach();
}

/// One hit per session that has any (#212 一期 shows one snippet per session, not every message in it), in no
/// particular order; empty (never `None`) while the index has not finished building yet. Stops after
/// [`MAX_CONTENT_MATCHES`] sessions.
pub fn search(query: &str, cx: &mut App) -> Vec<(String, Hit)> {
    let m = model(cx);
    let c = m.lock().unwrap();
    if c.state != BuildState::Ready {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (id, (_, items)) in c.by_session.iter() {
        if let Some(hit) = search_indexed(items, query).into_iter().next() {
            out.push((id.clone(), hit));
            if out.len() >= MAX_CONTENT_MATCHES {
                break;
            }
        }
    }
    out
}
