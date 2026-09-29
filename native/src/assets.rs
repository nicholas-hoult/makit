//! 打进二进制的静态资源（GPUI `AssetSource`），`svg().path("icons/xxx.svg")` 从这里取。
//!
//! 图标从 WebView 版组件里的内联 SVG 原样抽出来，放在 `native/assets/icons/`，文件名前缀写来源组件
//! （TRD #226「视觉一致」）。用 `include_bytes!` 而不是运行时读目录：打包后的 .app 里没有源码目录。
//!
//! **加一个资源**：文件放进 `native/assets/`，在下面 `FILES` 里自己包的那段加一行。

use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};

macro_rules! asset {
    ($path:literal) => {
        ($path, include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/", $path)) as &[u8])
    };
}

const FILES: &[(&str, &[u8])] = &[
    // ---- C 工作区（ContainerView.tsx / App.tsx 标题栏）----
    asset!("icons/container-new-terminal.svg"),
    asset!("icons/container-split-v.svg"),
    asset!("icons/container-split-h.svg"),
    asset!("icons/container-maximize.svg"),
    asset!("icons/container-restore.svg"),
    asset!("icons/titlebar-sidebar-toggle.svg"),
    asset!("icons/titlebar-bell.svg"),
    // ---- B 侧栏（SessionTree.tsx）----
    // 「显示选项」按钮的三横线：SessionTree.tsx:811 原样
    asset!("icons/session-tree-options.svg"),
    // 组 / 项目头的折叠箭头：TS 版是字符 › 转 90°，GPUI 转不了文字，照 › 的字形画成可旋转的 SVG
    asset!("icons/session-tree-chevron.svg"),
    // ---- D 浮层（CommandPalette.tsx；路径常量在 overlays::icons）----
    asset!("icons/search.svg"),
    asset!("icons/filter.svg"),
    // ⌘K 组头折叠箭头：▸ 画成同形三角
    asset!("icons/chevron-right.svg"),
    // ---- 工具 logo（tool_logo）：官方 favicon，claude 来自 anthropic.com，codex 来自 openai.com ----
    asset!("logos/claude.ico"),
    asset!("logos/codex.ico"),
    // ---- E 通知 ----
    asset!("icons/notif-bell.svg"),
    asset!("icons/notif-empty-bell-off.svg"),
];

/// 侧栏 / 工具选择器里的工具 logo：随程序打包的官方 favicon（和 Tauri 版第一次联网下载后缓存的是同一份文件）。
/// 不联网下载：那要把 tokio / reqwest / TLS 整套拖进来，和 GPUI 版「体积小、启动快」的目的相反；
/// 而且 openai.com 的 favicon 在脚本请求下会被 Cloudflare 挑战拦成 403。没有对应 logo 的工具返回 None，
/// 调用方退回文字徽章（codex 的「CX」、工具选择器的 ◆ / ⬡）
pub fn tool_logo(tool: &str) -> Option<gpui::ImageSource> {
    let path = match tool {
        "claude" => "logos/claude.ico",
        "codex" => "logos/codex.ico",
        _ => return None,
    };
    Some(gpui::ImageSource::Resource(gpui::Resource::Embedded(path.into())))
}

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(FILES.iter().find(|(p, _)| *p == path).map(|(_, b)| Cow::Borrowed(*b)))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(FILES.iter().filter(|(p, _)| p.starts_with(path)).map(|(p, _)| SharedString::from(*p)).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 打进去的 logo 必须真能被 GPUI 的图片加载器解出来（它用 image::guess_format 认格式），
    /// 而且不能太小：侧栏最大用到 16px，2x 屏要 32px。错了在界面上就是工具图标一块空白或糊成一团
    #[test]
    fn embedded_tool_logos_decode_and_are_big_enough() {
        for tool in ["claude", "codex"] {
            let Some(gpui::ImageSource::Resource(gpui::Resource::Embedded(path))) = tool_logo(tool) else { panic!("{tool} 没有内置 logo") };
            let bytes = Assets.load(&path).unwrap().unwrap_or_else(|| panic!("{path} 不在资源表里"));
            let img = image::load_from_memory(&bytes).unwrap_or_else(|e| panic!("{path} 解不出来：{e}"));
            assert!(img.width() >= 32 && img.height() >= 32, "{path} 只有 {}×{}", img.width(), img.height());
        }
        assert!(tool_logo("bash").is_none(), "没有 logo 的工具要让调用方走文字徽章");
    }

    #[test]
    fn every_asset_loads_and_is_svg() {
        for (p, bytes) in FILES {
            assert!(Assets.load(p).unwrap().is_some(), "{p}");
            if p.ends_with(".svg") {
                assert!(std::str::from_utf8(bytes).unwrap().contains("<svg"), "{p} 不是 svg");
            }
        }
        assert!(Assets.load("icons/没有这个.svg").unwrap().is_none());
        assert!(Assets.list("icons/").unwrap().len() >= 2);
        // C 工作区的图标是 ContainerView / 标题栏原来那几个 16×16 内联 SVG
        for (p, bytes) in FILES.iter().filter(|(p, _)| p.starts_with("icons/container-") || p.starts_with("icons/titlebar-")) {
            assert!(std::str::from_utf8(bytes).unwrap().contains("viewBox=\"0 0 16 16\""), "{p} 不是原来那个 16×16 的 svg");
        }
    }
}
