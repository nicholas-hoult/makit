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
//! `emphasis`. The tool logo gets a state-coloured ring when its session is open; unopened ones are dimmed instead.

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
/// only one allowed to stand out by colour as well as by shape. Waiting uses `t.attention`, not `t.warning`:
/// plain warning yellow turns muddy brown on light themes once pushed for contrast, which read as not
/// urgent at all (#278, found by a screen-recorded 0.2.12 pass over all four states).
fn hued(rs: RunState, t: &Theme, alpha: f32) -> Hsla {
    let mut c = match rs {
        RunState::Waiting => t.attention,
        RunState::Busy | RunState::Idle | RunState::Stopped => t.fg_muted,
    };
    c.a *= alpha;
    c
}

pub fn color(rs: RunState, t: &Theme) -> Hsla {
    hued(rs, t, emphasis(rs))
}

/// Whether the *standalone* icon (tab bar, or sidebar with "show icon" off) draws at all. Kept narrow on
/// purpose: without a tool logo to carry identity, idle / stopped have nothing worth taking the space for.
pub fn shown(rs: RunState) -> bool {
    matches!(rs, RunState::Waiting | RunState::Busy)
}

/// 状态颜色（#D-2302 改为颜色环，取代原来工具图标右下角的角标）：每种状态一个颜色，互不相同。
/// 等待用 `attention`（需要你），进行中用 `accent`（主题强调色，蓝），空闲用 `success`（绿），停止用灰。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StateHue {
    Attention,
    Accent,
    Success,
    Muted,
}

pub fn state_hue(rs: RunState) -> StateHue {
    match rs {
        RunState::Waiting => StateHue::Attention,
        RunState::Busy => StateHue::Accent,
        RunState::Idle => StateHue::Success,
        RunState::Stopped => StateHue::Muted,
    }
}

/// 状态环的颜色（#D-2302）：只在打开的会话上画，颜色就是状态色
pub fn ring_color(rs: RunState, t: &Theme) -> Hsla {
    match state_hue(rs) {
        StateHue::Attention => t.attention,
        StateHue::Accent => t.accent,
        StateHue::Success => t.success,
        StateHue::Muted => t.fg_muted,
    }
}

/// 打开的会话亮，没打开的整体变暗。只有亮的才区分状态（状态环只画在亮的上面）
pub fn logo_opacity(opened: bool) -> f32 {
    if opened { 1.0 } else { 0.4 }
}

/// 模型图标（或 12px 文字徽章）放在固定的 16x16 槽里。打开的会话外面画状态环，没打开的只变暗。
/// 槽的大小在所有状态下都一样，状态变化时标题不会跳
pub fn identity(rs: RunState, logo: AnyElement, t: &Theme, opened: bool) -> AnyElement {
    use gpui::{div, ParentElement};
    let slot = div()
        .relative()
        .w(px(16.))
        .h(px(16.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .child(div().opacity(logo_opacity(opened)).child(logo));
    if opened {
        slot.border_2().border_color(ring_color(rs, t)).into_any_element()
    } else {
        slot.into_any_element()
    }
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
        // waiting is the one state allowed a hue, because it is the one that needs you to act, and it uses
        // the dedicated `attention` colour, not `warning` (plain warning yellow goes muddy on light themes)
        let t = crate::theme::Theme::by_id("embark", &[]);
        assert_ne!(color(RunState::Waiting, &t).h, t.fg_muted.h);
        assert_eq!(color(RunState::Waiting, &t).h, t.attention.h, "waiting should use the dedicated attention colour");
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
    fn each_state_has_its_own_hue_so_the_ring_tells_them_apart() {
        // 底色块（替代原来的右下角角标）：四种状态各用一个颜色，互不相同，才能一眼分清
        let hues: std::collections::HashSet<_> = [RunState::Waiting, RunState::Busy, RunState::Idle, RunState::Stopped].into_iter().map(state_hue).collect();
        assert_eq!(hues.len(), 4, "every state needs a hue of its own");
    }

    #[test]
    fn unopened_sessions_dim_their_logo_and_opened_ones_stay_bright() {
        assert_eq!(logo_opacity(true), 1.0, "打开的会话亮");
        assert!(logo_opacity(false) < 1.0, "没打开的会话暗");
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
