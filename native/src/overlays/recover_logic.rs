//! 恢复 cwd 对话框的纯逻辑（#217 的改进点落在这里）。
//!
//! #217：「指到新位置」的输入框原来**预填了那个已经不存在的原目录**，直接点「指过去并打开」必然失败，
//! 报的还是内部函数的措辞（`目标目录不存在: …`）。现在：
//! 1. 输入框不预填（留空 + placeholder），旁边给一个「选择目录…」走系统选择框；
//! 2. 边输边校验：不是已存在的目录时按钮置灰、下面直接说原因，不用点了再报错；
//! 4. 报错改成面向用户的话（`friendly_error`）。
//! （第 3 点「启动恢复时不要每次都弹」归触发方 A / C 包，这里只保证同一时间只弹一个。）

use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RelinkCheck {
    /// 还没填：按钮置灰，不显示提示
    Empty,
    /// 填了但不能用：按钮置灰，显示原因
    Invalid(String),
    /// 可以用：展开 `~` 之后的绝对路径
    Ok(String),
}

/// 校验「指到新位置」的输入。`home` 用来展开 `~`（测试注入）
pub fn check_relink_target(input: &str, home: Option<&Path>) -> RelinkCheck {
    let t = input.trim();
    if t.is_empty() {
        return RelinkCheck::Empty;
    }
    let home = home.map(|h| h.to_path_buf()).or_else(dirs::home_dir);
    let expanded = if t == "~" {
        home.map(|h| h.display().to_string())
    } else if let Some(rest) = t.strip_prefix("~/") {
        home.map(|h| h.join(rest).display().to_string())
    } else {
        Some(t.to_string())
    };
    let Some(mut p) = expanded else { return RelinkCheck::Invalid("找不到 home 目录，请填完整路径".into()) };
    if !p.starts_with('/') {
        return RelinkCheck::Invalid("请填绝对路径（以 / 或 ~ 开头）".into());
    }
    while p.len() > 1 && p.ends_with('/') {
        p.pop();
    }
    let path = Path::new(&p);
    if path.is_dir() {
        RelinkCheck::Ok(p)
    } else if path.exists() {
        RelinkCheck::Invalid("这是一个文件，不是目录".into())
    } else {
        RelinkCheck::Invalid("这个目录也不存在，请选一个已有的目录".into())
    }
}

/// 后端（makit_core::recovery）的报错 → 面向用户的话
pub fn friendly_error(e: &str) -> String {
    if e.starts_with("目标目录不存在") {
        "这个目录也不存在，请选一个已有的目录".into()
    } else {
        e.to_string()
    }
}

/// 对话框文案分两种：按 cwd 索引的（claude）和不按目录索引的（codex，storage_folder 为空）。
/// 会话还没加载到时按 claude 讲（绝大多数情况）
pub fn keyed_by_cwd(storage_folder: Option<&str>) -> bool {
    storage_folder.map(|s| !s.is_empty()).unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("makit-recover-test-{}-{name}", std::process::id()));
        let _ = std::fs::create_dir_all(&d);
        d
    }

    #[test]
    fn empty_input_just_disables() {
        assert_eq!(check_relink_target("", None), RelinkCheck::Empty);
        assert_eq!(check_relink_target("   ", None), RelinkCheck::Empty);
    }

    #[test]
    fn missing_dir_is_explained_before_clicking() {
        let gone = tmp("gone").join("不存在");
        let RelinkCheck::Invalid(msg) = check_relink_target(gone.to_str().unwrap(), None) else { panic!() };
        assert!(msg.contains("不存在") && msg.contains("已有的目录"), "{msg}");
        assert!(!msg.contains("目标目录不存在:"), "不暴露内部措辞");
    }

    #[test]
    fn relative_path_and_file_are_rejected() {
        assert!(matches!(check_relink_target("code/proj", None), RelinkCheck::Invalid(m) if m.contains("绝对路径")));
        let d = tmp("file");
        let f = d.join("a.txt");
        std::fs::write(&f, "x").unwrap();
        assert!(matches!(check_relink_target(f.to_str().unwrap(), None), RelinkCheck::Invalid(m) if m.contains("文件")));
    }

    #[test]
    fn existing_dir_ok_with_tilde_and_trim() {
        let d = tmp("ok");
        assert_eq!(check_relink_target(&format!("  {}  ", d.display()), None), RelinkCheck::Ok(d.display().to_string()));
        let home = d.parent().unwrap();
        let name = d.file_name().unwrap().to_str().unwrap();
        assert_eq!(check_relink_target(&format!("~/{name}"), Some(home)), RelinkCheck::Ok(d.display().to_string()), "~ 展开成 home");
        assert_eq!(check_relink_target("~", Some(home)), RelinkCheck::Ok(home.display().to_string()));
        // 结尾的 / 去掉（和会话记录里的 cwd 写法一致）
        assert_eq!(check_relink_target(&format!("{}/", d.display()), None), RelinkCheck::Ok(d.display().to_string()));
    }

    #[test]
    fn friendly_errors() {
        assert!(friendly_error("目标目录不存在: /a/b").contains("请选一个已有的目录"));
        assert!(!friendly_error("目标目录不存在: /a/b").contains("目标目录不存在:"));
        assert_eq!(friendly_error("父目录也不存在：/x。…"), "父目录也不存在：/x。…", "本来就是给人看的原样保留");
    }

    #[test]
    fn codex_is_not_keyed_by_cwd() {
        assert!(keyed_by_cwd(None));
        assert!(keyed_by_cwd(Some("-Users-me-p")));
        assert!(!keyed_by_cwd(Some("")));
    }
}
