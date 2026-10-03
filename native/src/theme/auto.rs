//! 首次启动按系统外观选默认主题（#256 A6）。
//!
//! 规则：**用户没有主动选过主题（`chosen == false`）就一直跟着系统外观**——浅色用 GitHub Light Default，深色用 Embark；
//! 一旦主动选过（设置里选主题、导入主题）就固定。老用户的状态文件里没有 `chosen`，读出来按「选过」算，外观不会被改。
//!
//! 为什么单独测：这条规则错了，要么新用户在浅色系统里被一块黑底迎接，要么老用户升级后主题莫名其妙变了。
//! `MAKIT_APPEARANCE=light|dark` 可以在 dev 里覆盖系统外观复现（不用真去切系统设置）。

use gpui::{App, Entity, WindowAppearance};

use crate::state::AppState;

use super::builtin::DEFAULT_THEME_ID;

/// 系统是浅色外观时的默认主题（可读性测试已通过，最差文字色 3.2:1）
pub const LIGHT_DEFAULT_ID: &str = "github-light-default";

/// 没主动选过时，按系统外观该用哪个内置主题
pub fn auto_theme_id(dark: bool) -> &'static str {
    if dark { DEFAULT_THEME_ID } else { LIGHT_DEFAULT_ID }
}

/// 实际生效的主题 id：选过就用保存的，没选过就按系统外观
pub fn effective_theme_id<'a>(chosen: bool, id: &'a str, dark: bool) -> &'a str {
    if chosen { id } else { auto_theme_id(dark) }
}

/// GPUI 的四种外观（Light / VibrantLight / Dark / VibrantDark）折成「是不是深色」
pub fn is_dark(a: WindowAppearance) -> bool {
    matches!(a, WindowAppearance::Dark | WindowAppearance::VibrantDark)
}

/// `MAKIT_APPEARANCE` 的值：`light` / `dark`（忽略大小写和空白），别的都当没设
pub fn parse_override(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "light" => Some(false),
        "dark" => Some(true),
        _ => None,
    }
}

/// 按当前偏好和系统外观算出该用的主题并换上（启动、系统外观变化、设置里改主题之后都调它）
pub fn refresh(state: &Entity<AppState>, cx: &mut App) {
    let t = state.read(cx).prefs.theme.clone();
    let dark = system_is_dark(cx);
    super::set_theme(effective_theme_id(t.chosen, &t.id, dark), &t.imported, cx);
}

/// 现在系统是不是深色外观（`MAKIT_APPEARANCE` 优先，给 dev 复现用）
pub fn system_is_dark(cx: &App) -> bool {
    std::env::var("MAKIT_APPEARANCE").ok().and_then(|v| parse_override(&v)).unwrap_or_else(|| is_dark(cx.window_appearance()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::builtin::builtin_themes;
    use crate::theme::derive::is_light;

    #[test]
    fn unchosen_follows_the_system_appearance() {
        assert_eq!(effective_theme_id(false, "dracula", true), DEFAULT_THEME_ID, "深色系统 → Embark");
        assert_eq!(effective_theme_id(false, "dracula", false), LIGHT_DEFAULT_ID, "浅色系统 → GitHub Light Default");
        assert_eq!(effective_theme_id(false, "", false), LIGHT_DEFAULT_ID);
    }

    /// 为什么要测：选过的主题不能被外观盖掉，否则用户明明选了某个主题，系统一切深浅色它就变了
    #[test]
    fn chosen_theme_ignores_the_system_appearance() {
        assert_eq!(effective_theme_id(true, "nord", true), "nord");
        assert_eq!(effective_theme_id(true, "nord", false), "nord");
        assert_eq!(effective_theme_id(true, "imported:我的", false), "imported:我的");
    }

    #[test]
    fn the_two_auto_themes_exist_and_have_the_right_brightness() {
        let all = builtin_themes();
        let dark = all.iter().find(|t| t.id == auto_theme_id(true)).expect("深色默认主题要在内置列表里");
        let light = all.iter().find(|t| t.id == auto_theme_id(false)).expect("浅色默认主题要在内置列表里（改名后默认悄悄回退到深色就是这条红）");
        assert!(!is_light(&dark.bg), "深色默认要真的是深色");
        assert!(is_light(&light.bg), "浅色默认要真的是浅色");
    }

    #[test]
    fn all_four_gpui_appearances_fold_to_light_or_dark() {
        assert!(!is_dark(WindowAppearance::Light) && !is_dark(WindowAppearance::VibrantLight));
        assert!(is_dark(WindowAppearance::Dark) && is_dark(WindowAppearance::VibrantDark));
    }

    #[test]
    fn appearance_override_parsing() {
        assert_eq!(parse_override("light"), Some(false));
        assert_eq!(parse_override(" DARK "), Some(true));
        assert_eq!(parse_override("乱写"), None);
        assert_eq!(parse_override(""), None);
    }
}
