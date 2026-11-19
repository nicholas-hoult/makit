//! Cross-session text search index for ⌘K's content-match group (#212, 总管 #D-2132).
//!
//! Built lazily on the first search, not at startup: scanning every session's transcript once (`open_transcript`
//! + a full `poll()` per session) is real work -- fine to pay once when someone actually searches content, wrong
//! to pay on every launch whether anyone ever searches or not. Runs on the background executor; the palette just
//! shows name matches only until it is ready, same degraded-but-correct state a fresh launch is already in.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use gpui::{App, Global};
use makit_core::search::{search_items, Hit};
use makit_core::transcript::Item;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BuildState {
    NotStarted,
    Building,
    Ready,
}

struct Corpus {
    state: BuildState,
    by_session: HashMap<String, Vec<Item>>,
}

struct Model(Arc<Mutex<Corpus>>);
impl Global for Model {}

fn model(cx: &mut App) -> Arc<Mutex<Corpus>> {
    if cx.try_global::<Model>().is_none() {
        cx.set_global(Model(Arc::new(Mutex::new(Corpus { state: BuildState::NotStarted, by_session: HashMap::new() }))));
    }
    cx.global::<Model>().0.clone()
}

/// Kicks off the one-time background build the first time this is called for real; every call after that is a
/// cheap no-op (whether still building or already done), so callers can just call this unconditionally once per
/// keystroke that has a non-empty query.
pub fn ensure_built(session_ids: Vec<String>, cx: &mut App) {
    let m = model(cx);
    {
        let mut c = m.lock().unwrap();
        if c.state != BuildState::NotStarted {
            return;
        }
        c.state = BuildState::Building;
    }
    cx.spawn(async move |cx| {
        let built = cx
            .background_executor()
            .spawn(async move {
                let mut map = HashMap::new();
                for id in session_ids {
                    let Some(mut reader) = makit_core::sessions::open_transcript(&id) else { continue };
                    if reader.poll().is_err() {
                        continue;
                    }
                    let items: Vec<Item> = (0..reader.len()).map(|i| reader.item(i).clone()).collect();
                    map.insert(id, items);
                }
                map
            })
            .await;
        let _ = cx.update(|cx| {
            let m = model(cx);
            {
                let mut c = m.lock().unwrap();
                c.by_session = built;
                c.state = BuildState::Ready;
            }
            // The palette only re-renders on its own query-change / state-change notifications; the index
            // finishing well after the keystroke that triggered it needs its own nudge to show up.
            cx.refresh_windows();
        });
    })
    .detach();
}

/// One hit per session that has any (#212 一期 shows one snippet per session, not every message in it), in no
/// particular order; empty (never `None`) while the index has not finished building yet.
pub fn search(query: &str, cx: &mut App) -> Vec<(String, Hit)> {
    let m = model(cx);
    let c = m.lock().unwrap();
    if c.state != BuildState::Ready {
        return Vec::new();
    }
    c.by_session.iter().filter_map(|(id, items)| search_items(items, query).into_iter().next().map(|hit| (id.clone(), hit))).collect()
}
