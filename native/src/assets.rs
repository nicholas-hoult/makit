//! 内嵌资源（GPUI `AssetSource`）：`native/assets/` 下的图标，编进二进制（打包不用带资源目录）。
//!
//! 图标是从 Tauri 版的内联 SVG 原样抽出来的（#226「视觉一致」），文件头注释写着来源。
//! **加一个图标**：文件放进 `native/assets/icons/`，在 `FILES` 里加一行；视图里 `svg().path("icons/xxx.svg")`。
//! （E 包先建的这个文件；其他包的图标照样往 `FILES` 里追加。）

use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};

const FILES: &[(&str, &[u8])] = &[
    // ---- E 通知 ----
    ("icons/notif-bell.svg", include_bytes!("../assets/icons/notif-bell.svg")),
    ("icons/notif-empty-bell-off.svg", include_bytes!("../assets/icons/notif-empty-bell-off.svg")),
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
    fn every_icon_loads_and_is_svg() {
        for (p, _) in FILES {
            let b = Assets.load(p).unwrap().expect(p);
            let s = std::str::from_utf8(&b).unwrap();
            assert!(s.contains("<svg") && s.contains("来源："), "{p} 要是 svg 且写明来源");
        }
        assert!(Assets.load("icons/没有.svg").unwrap().is_none());
    }
}
