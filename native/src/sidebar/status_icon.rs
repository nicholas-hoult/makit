//! Status look shared by the sidebar rows and the pane tabs (#245, redesigned in #290 / #278).
//!
//! Why a leaf module with its own tests: the two places used to speak different status languages, and the
//! later shared 8px dots told four states apart by colour alone (waiting / busy / idle were three similar
//! small blobs, the one that needs you was the least noticeable). Now every state has its own silhouette:
//!   waiting = filled disc with three dots cut out, busy = ring with a bright arc,
//!   idle = ring with a tick, stopped = dashed ring.
//! Nothing moves: any motion has to be a repaint of the whole window (GPUI has no partial redraw, and an animated
//! image is replayed by GPUI itself with a repaint per display frame), which measured ~28% of a core in the shipped
//! app with a few running sessions. The silhouettes already carry the meaning.
//! A wrong table here shows up as tabs and sidebar disagreeing about the same session, or as two states
//! drawing the same shape.
//!
//! Colour (#278, read off cmux's `SidebarCompactStatusGlyph`): only the state that asks you to do something
//! (waiting) gets a hue; everything settled — busy, idle, stopped — is grayscale, told apart by shape and
//! `emphasis`. The corner badge on the tool logo follows the same source's rule ("everything settled gets a
//! symbol that says what it is"): idle used to draw no badge at all, which read identical to a never-opened
//! session; it now gets the same tick-in-a-ring as the standalone icon, just shrunk to badge size.

use gpui::{px, svg, AnyElement, Hsla, IntoElement, Styled};

use super::groups::RunState;
use crate::theme::Theme;

/// Asset path of the icon for a state (registered in `crate::assets`)
pub fn path(rs: RunState) -> &'static str {
    match rs {
        RunState::Waiting => "icons/status-waiting.svg",
        RunState::Busy => "icons/status-busy.svg",
        RunState::Idle => "icons/status-idle.svg",
        RunState::Stopped => "icons/status-stopped.svg",
    }
}

/// How loud a state is drawn (alpha factor). Only the state that needs the user is at full strength; the rest recede
/// so a sidebar of live sessions does not read as a row of equally loud icons (#303).
pub fn emphasis(rs: RunState) -> f32 {
    match rs {
        RunState::Waiting => 1.0,
        RunState::Busy => 0.75,
        RunState::Idle => 0.5,
        RunState::Stopped => 0.45,
    }
}

/// Waiting is the only state with its own hue; busy / idle / stopped are all grayscale (#278) — a sidebar of
/// live sessions should not read as a row of equally loud colours, and the one state that needs you is the
/// only one allowed to stand out by colour as well as by shape.
pub fn color(rs: RunState, t: &Theme) -> Hsla {
    let mut c = match rs {
        RunState::Waiting => t.warning,
        RunState::Busy | RunState::Idle | RunState::Stopped => t.fg_muted,
    };
    c.a *= emphasis(rs);
    c
}

/// Whether the *standalone* icon (tab bar, or sidebar with "show icon" off) draws at all. Kept narrow on
/// purpose: without a tool logo to carry identity, idle / stopped have nothing worth taking the space for.
/// The corner badge on the tool logo (`badge`, below) is the wider one — see its doc comment.
pub fn shown(rs: RunState) -> bool {
    matches!(rs, RunState::Waiting | RunState::Busy)
}

/// The corner badge on a tool logo (#303, widened in #278). Every state draws one: idle / stopped used to
/// draw nothing, which made a live-but-quiet session look identical to one that was never opened — the
/// cmux principle this followed is "everything settled gets a symbol that says what it is", not a blank.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Badge {
    Dot,
    Ring,
    Check,
    Dashed,
}

pub fn badge(rs: RunState) -> Option<Badge> {
    Some(match rs {
        RunState::Waiting => Badge::Dot,
        RunState::Busy => Badge::Ring,
        RunState::Idle => Badge::Check,
        RunState::Stopped => Badge::Dashed,
    })
}

/// A stopped session keeps its logo (it still says who it is) but recedes
pub fn identity_opacity(rs: RunState) -> f32 {
    if rs == RunState::Stopped { 0.45 } else { 1.0 }
}

/// Tool logo (or text badge, 12px: the same size as the title text) in a fixed 16x16 slot with the state badge on its lower-right corner.
/// The slot has a fixed width in every state, so titles stay aligned when a session changes state.
pub fn identity(rs: RunState, logo: AnyElement, t: &Theme) -> AnyElement {
    use gpui::{div, ParentElement};
    use gpui::prelude::FluentBuilder;
    // The slot is as tall as the title line (16) and the logo is centred in it, so logo and title share a centre line
    div()
        .relative()
        .w(px(16.))
        .h(px(16.))
        .flex_none()
        .flex()
        .items_center()
        .child(div().opacity(identity_opacity(rs)).child(logo))
        .when_some(badge(rs), |d, b| {
            let c = color(rs, t);
            // logo is 12 wide / tall, centred in 16: its lower-right corner is at (12, 14), the badge sits on it.
            // Dot / Ring are plain shapes at the original 5px; Check / Dashed reuse the standalone SVGs (they
            // need the extra detail to read at a glance) at 7px, nudged to keep the same corner alignment.
            match b {
                Badge::Dot => d.child(div().absolute().right(px(2.)).bottom(px(1.)).size(px(5.)).rounded_full().bg(c)),
                Badge::Ring => d.child(div().absolute().right(px(2.)).bottom(px(1.)).size(px(5.)).rounded_full().border_1().border_color(c)),
                Badge::Check => d.child(svg().path(path(RunState::Idle)).absolute().right(px(1.)).bottom(px(0.)).size(px(7.)).text_color(c)),
                Badge::Dashed => d.child(svg().path(path(RunState::Stopped)).absolute().right(px(1.)).bottom(px(0.)).size(px(7.)).text_color(c)),
            }
        })
        .into_any_element()
}

/// The icon itself, `size` px square. Sidebar rows use 12, tabs 11. A state that is not `shown` is an empty box of the
/// same size, so titles stay aligned and do not jump when a session changes state.
pub fn render(rs: RunState, size: f32, t: &Theme) -> AnyElement {
    if !shown(rs) {
        return gpui::div().size(px(size)).flex_none().into_any_element();
    }
    svg().path(path(rs)).size(px(size)).flex_none().text_color(color(rs, t)).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::AssetSource;

    const ALL: [RunState; 4] = [RunState::Waiting, RunState::Busy, RunState::Idle, RunState::Stopped];

    #[test]
    fn every_state_has_its_own_registered_icon() {
        let mut seen = std::collections::HashSet::new();
        for rs in ALL {
            assert!(seen.insert(path(rs)), "two states share the icon {}", path(rs));
            assert!(crate::assets::Assets.load(path(rs)).unwrap().is_some(), "{} is not registered in assets.rs", path(rs));
        }
    }

    #[test]
    fn only_the_state_that_needs_you_is_at_full_strength() {
        // Every live session wears an icon, so the quiet states must recede and the one that needs you must stay
        // the most noticeable. A flat table brings back a sidebar full of equally loud icons (#303).
        assert_eq!(emphasis(RunState::Waiting), 1.0);
        assert!(emphasis(RunState::Busy) < emphasis(RunState::Waiting));
        assert!(emphasis(RunState::Idle) < emphasis(RunState::Busy));
        assert!(emphasis(RunState::Stopped) <= emphasis(RunState::Idle));
    }

    #[test]
    fn only_waiting_has_its_own_hue() {
        // #278: busy / idle / stopped are all grayscale, told apart by shape + emphasis, not by colour;
        // waiting is the one state allowed a hue, because it is the one that needs you to act
        let t = crate::theme::Theme::by_id("embark", &[]);
        assert_ne!(color(RunState::Waiting, &t).h, t.fg_muted.h);
        for rs in [RunState::Busy, RunState::Idle, RunState::Stopped] {
            assert_eq!(color(rs, &t).h, t.fg_muted.h, "{rs:?} should be grayscale");
            assert_eq!(color(rs, &t).s, t.fg_muted.s, "{rs:?} should be grayscale");
        }
    }

    #[test]
    fn only_states_that_ask_for_attention_get_a_standalone_icon() {
        // Narrow on purpose (`shown`'s doc comment): without a tool logo to anchor identity, idle / stopped
        // have nothing worth the space in the tab bar or the logo-off sidebar fallback
        assert!(shown(RunState::Waiting) && shown(RunState::Busy));
        assert!(!shown(RunState::Idle) && !shown(RunState::Stopped));
    }

    #[test]
    fn badge_draws_something_for_every_live_state() {
        // #278: idle / stopped used to have no badge at all (indistinguishable from a never-opened session);
        // cmux's rule is "everything settled gets a symbol", so every state now gets its own badge shape
        assert_eq!(badge(RunState::Waiting), Some(Badge::Dot));
        assert_eq!(badge(RunState::Busy), Some(Badge::Ring));
        assert_eq!(badge(RunState::Idle), Some(Badge::Check));
        assert_eq!(badge(RunState::Stopped), Some(Badge::Dashed));
        let shapes: std::collections::HashSet<_> = [Badge::Dot, Badge::Ring, Badge::Check, Badge::Dashed].into_iter().collect();
        assert_eq!(shapes.len(), 4, "every state must have a shape of its own");
    }

    #[test]
    fn only_a_stopped_session_dims_its_logo() {
        assert!(identity_opacity(RunState::Stopped) < 1.0);
        for rs in [RunState::Waiting, RunState::Busy, RunState::Idle] {
            assert_eq!(identity_opacity(rs), 1.0);
        }
    }

    #[test]
    fn nothing_requests_a_repaint_tick() {
        // The pulse clock repaints the whole window when anyone asks for ticks. Drawing an icon must never ask (#290).
        let _ = crate::pulse::take_want();
        let t = crate::theme::Theme::by_id("embark", &[]);
        for rs in ALL {
            let _ = render(rs, 14., &t);
        }
        assert!(!crate::pulse::take_want(), "drawing a status icon asked for repaint ticks");
    }
}
