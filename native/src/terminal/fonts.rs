//! 终端字体：对标 对标产品（对标终端 内核）。确切参数取自 对标终端 `b1d2b7e` / 对标产品 `da27bbc`（2026-09-28）：
//! - 主字体 JetBrains Mono（对标终端 内置；这里随程序打包静态四款，OFL 许可见 `assets/fonts/OFL.txt`），13pt
//! - 中日韩：对标产品 按系统首选语言把 CJK 范围固定映射到一款字体（`对标产品的 CJK 字体映射`）——
//!   ja → Hiragino Sans、zh-Hant / zh-TW / zh-HK → PingFang TC、其他 zh → PingFang SC。
//!   不让系统按评分挑回退字体（评分偏好等宽，常挑中装饰字体；旧栈把韩文字体 Apple SD Gothic Neo 排第一，
//!   汉字有的走它、有的走苹方 → 同一行「有大有小」）。GPUI 的回退是有序列表不能按范围映射，
//!   但 JetBrains Mono 本身没有 CJK 字形，把映射字体排第一效果相同
//! - 格子：`Metrics.zig` 在设备像素上 `@round(face_width)` / `@round(ascent - descent + line_gap)`
//!
//! 为什么单独测：错了在界面上就是中文忽大忽小、字距不齐，或同样宽的 pane 列数和 对标产品 对不上。

use std::borrow::Cow;

/// 终端主字体栈：第一个能加载的生效
pub const FONT_STACK: &[&str] = &["JetBrains Mono", "SF Mono", ".AppleSystemUIFontMonospaced", "Menlo"];

/// 随程序打包的字体（启动时 `add_fonts`）
pub fn embedded_fonts() -> Vec<Cow<'static, [u8]>> {
    vec![
        Cow::Borrowed(include_bytes!("../../assets/fonts/JetBrainsMono-Regular.ttf").as_slice()),
        Cow::Borrowed(include_bytes!("../../assets/fonts/JetBrainsMono-Bold.ttf").as_slice()),
        Cow::Borrowed(include_bytes!("../../assets/fonts/JetBrainsMono-Italic.ttf").as_slice()),
        Cow::Borrowed(include_bytes!("../../assets/fonts/JetBrainsMono-BoldItalic.ttf").as_slice()),
    ]
}

/// The system font that covers each CJK language on macOS, keyed by lowercase BCP 47 prefix. Order matters:
/// Traditional Chinese (script `hant`, regions TW / HK) has to be tried before the bare `zh`.
const CJK_FONT_BY_PREFIX: &[(&[&str], &str)] = &[
    (&["ja"], "Hiragino Sans"),
    (&["zh-hant", "zh-tw", "zh-hk"], "PingFang TC"),
    (&["zh"], "PingFang SC"),
];

/// CJK fallback fonts in the order of the system's preferred languages (`CFLocaleCopyPreferredLanguages`): the
/// first CJK language picks the primary font, the rest follow as a safety net. Without any CJK language the
/// primary one is Simplified Chinese.
pub fn cjk_fallbacks(preferred_languages: &[String]) -> Vec<&'static str> {
    let first = preferred_languages.iter().find_map(|lang| {
        let lang = lang.to_ascii_lowercase();
        CJK_FONT_BY_PREFIX.iter().find(|(prefixes, _)| prefixes.iter().any(|p| lang.starts_with(p))).map(|(_, font)| *font)
    });
    let mut out = vec![first.unwrap_or("PingFang SC")];
    for f in ["PingFang SC", "PingFang TC", "Hiragino Sans", "Apple SD Gothic Neo"] {
        if !out.contains(&f) {
            out.push(f);
        }
    }
    out
}

/// 系统首选语言（系统设置「语言与地区」的顺序），同 对标产品 用的 `Locale.preferredLanguages`
#[cfg(target_os = "macos")]
pub fn system_preferred_languages() -> Vec<String> {
    use core_foundation_sys::array::{CFArrayGetCount, CFArrayGetValueAtIndex};
    use core_foundation_sys::base::CFRelease;
    use core_foundation_sys::locale::CFLocaleCopyPreferredLanguages;
    use core_foundation_sys::string::{kCFStringEncodingUTF8, CFStringGetCString, CFStringRef};
    let mut out = Vec::new();
    // SAFETY：Copy 规则拿到的数组自己 CFRelease；元素是数组持有的 CFString，只读不释放
    unsafe {
        let arr = CFLocaleCopyPreferredLanguages();
        if arr.is_null() {
            return out;
        }
        for i in 0..CFArrayGetCount(arr) {
            let s = CFArrayGetValueAtIndex(arr, i) as CFStringRef;
            let mut buf = [0i8; 64];
            if !s.is_null() && CFStringGetCString(s, buf.as_mut_ptr(), buf.len() as _, kCFStringEncodingUTF8) != 0 {
                out.push(std::ffi::CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned());
            }
        }
        CFRelease(arr as _);
    }
    out
}

/// System preferred languages outside macOS: the locale environment variables (`LC_ALL`, `LC_MESSAGES`, `LANG`),
/// `zh_CN.UTF-8` becomes `zh-CN`. Windows has no such variables by default, so it falls back to English until a
/// `GetUserPreferredUILanguages` backend is written (platform matrix P20).
#[cfg(not(target_os = "macos"))]
pub fn system_preferred_languages() -> Vec<String> {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|k| std::env::var(k).ok())
        .map(|v| v.split(['.', '@']).next().unwrap_or("").replace('_', "-"))
        .filter(|v| !v.is_empty() && v != "C" && v != "POSIX")
        .collect()
}

/// 格子宽高（逻辑像素）：对标终端 在设备像素上四舍五入。`advance` / `ascent` / `descent`（正数）/ `line_gap`
/// 是逻辑像素下的字体度量，`scale` 是 backing scale
pub fn cell_size(advance: f32, ascent: f32, descent: f32, line_gap: f32, scale: f32) -> (f32, f32) {
    let scale = scale.max(1.0);
    let w = (advance * scale).round().max(1.0) / scale;
    let h = ((ascent + descent + line_gap) * scale).round().max(1.0) / scale;
    (w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn langs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn cjk_font_follows_first_cjk_language_like_reference() {
        assert_eq!(cjk_fallbacks(&langs(&["zh-Hans-CN", "en-US"]))[0], "PingFang SC");
        assert_eq!(cjk_fallbacks(&langs(&["en-US", "zh-Hant-TW"]))[0], "PingFang TC");
        assert_eq!(cjk_fallbacks(&langs(&["zh-HK"]))[0], "PingFang TC");
        assert_eq!(cjk_fallbacks(&langs(&["ja-JP", "zh-Hans"]))[0], "Hiragino Sans");
        assert_eq!(cjk_fallbacks(&langs(&["en-US"]))[0], "PingFang SC", "没有 CJK 语言按简体中文");
    }

    #[test]
    fn korean_font_is_never_first_and_no_duplicates() {
        for l in [vec!["zh-Hans"], vec!["ja"], vec!["en"], vec![]] {
            let f = cjk_fallbacks(&langs(&l));
            assert_ne!(f[0], "Apple SD Gothic Neo", "{l:?}：韩文字体排第一会让汉字忽大忽小");
            let mut d = f.clone();
            d.dedup();
            assert_eq!(d.len(), f.len());
        }
    }

    #[test]
    fn cell_size_rounds_in_device_pixels_like_reference() {
        // JetBrains Mono 13pt @2x：advance 7.8 → 15.6 设备像素 → 16 → 8.0；高 (12.74+3.9)*2=33.28 → 33 → 16.5
        assert_eq!(cell_size(7.8, 12.74, 3.9, 0.0, 2.0), (8.0, 16.5));
        // 旧算法宽向下取整会得 7.5，高向上取整会得 17.0：两个都和 对标终端 不一样
        assert_eq!(cell_size(7.74, 12.0, 4.0, 0.0, 2.0).0, 7.5, "15.48 → 15 → 7.5（四舍五入，不是向上）");
        assert_eq!(cell_size(8.0, 12.0, 4.0, 1.0, 1.0), (8.0, 17.0), "line_gap 计入行高");
    }
}
