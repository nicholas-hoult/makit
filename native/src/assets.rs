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
];

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
    }
}
