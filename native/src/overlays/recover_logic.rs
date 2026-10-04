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

/// 下拉里的一项
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Choice {
    /// 指到一个已有目录（`why` 说明这个候选是怎么来的）
    Relink { path: String, why: &'static str },
    /// 在原路径重建空目录
    Recreate,
    /// 打开系统目录选择框
    Pick,
}

/// 启动目录不在时下拉里给哪些选项（#239，照 Claude Code 的列表选择）：
/// 会话最近待过的目录 → 原目录往上最近一个还在的祖先 → 主目录，然后是「重建原目录」「选择其他目录…」。
/// 候选之间去重；`is_dir` 测试时注入
pub fn choices(orig: &str, last_cwd: Option<&str>, home: Option<&str>, is_dir: &dyn Fn(&str) -> bool) -> Vec<Choice> {
    let mut out: Vec<Choice> = Vec::new();
    let add = |path: &str, why: &'static str, out: &mut Vec<Choice>| {
        let dup = out.iter().any(|c| matches!(c, Choice::Relink { path: p, .. } if p == path));
        if !path.is_empty() && path != orig && !dup && is_dir(path) {
            out.push(Choice::Relink { path: path.to_string(), why });
        }
    };
    if let Some(l) = last_cwd {
        add(l, "会话最近待过的目录", &mut out);
    }
    // 原目录往上最近一个还在的祖先（不给文件系统根：不是有意义的工作目录）
    let mut cur = Path::new(orig).parent();
    while let Some(p) = cur {
        let s = p.to_string_lossy();
        if s == "/" || s.is_empty() {
            break;
        }
        if is_dir(&s) {
            add(&s, "上级目录", &mut out);
            break;
        }
        cur = p.parent();
    }
    if let Some(h) = home {
        add(h, "主目录", &mut out);
    }
    out.push(Choice::Recreate);
    out.push(Choice::Pick);
    out
}

/// 方向键移动选中项，首尾循环
pub fn step(sel: usize, len: usize, delta: i32) -> usize {
    if len == 0 {
        return 0;
    }
    (sel as i64 + delta as i64).rem_euclid(len as i64) as usize
}

/// 恢复过的会话记住的目录：在选择器里选定的目录优先，没有就用 jsonl 里最后的 cwd。
/// 为什么要测：只看 jsonl 的话，选「上级目录」之后会话要再说一句话才会记住，侧栏再打开又弹选择器
pub fn remembered_dir<'a>(chosen: Option<&'a str>, last_cwd: &'a str) -> &'a str {
    chosen.filter(|s| !s.is_empty()).unwrap_or(last_cwd)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chosen_directory_wins_over_last_cwd() {
        assert_eq!(remembered_dir(Some("/a/上级"), "/a/项目"), "/a/上级");
    }

    #[test]
    fn falls_back_to_last_cwd_without_a_choice() {
        assert_eq!(remembered_dir(None, "/a/项目"), "/a/项目");
        assert_eq!(remembered_dir(Some(""), "/a/项目"), "/a/项目", "空的选择不算");
        assert_eq!(remembered_dir(None, ""), "", "什么都没有：返回空，调用方不走静默路径");
    }

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

    fn dirs(list: &[&'static str]) -> impl Fn(&str) -> bool {
        let set: Vec<String> = list.iter().map(|s| s.to_string()).collect();
        move |p: &str| set.iter().any(|s| s == p)
    }

    fn relink(path: &str, why: &'static str) -> Choice {
        Choice::Relink { path: path.into(), why }
    }

    #[test]
    fn choices_prefer_last_cwd_then_nearest_ancestor_then_home() {
        let is_dir = dirs(&["/w/a", "/w", "/Users/me", "/w/a/b"]);
        let got = choices("/w/a/b/c/d", Some("/w/a"), Some("/Users/me"), &is_dir);
        assert_eq!(
            got,
            vec![
                relink("/w/a", "会话最近待过的目录"),
                relink("/w/a/b", "上级目录"),
                relink("/Users/me", "主目录"),
                Choice::Recreate,
                Choice::Pick,
            ]
        );
    }

    #[test]
    fn choices_skip_missing_and_duplicate_candidates() {
        // last_cwd 也不在了、和原目录相同、祖先就是主目录：不重复列
        let is_dir = dirs(&["/Users/me"]);
        let got = choices("/Users/me/proj/x", Some("/Users/me/proj/x"), Some("/Users/me"), &is_dir);
        assert_eq!(got, vec![relink("/Users/me", "上级目录"), Choice::Recreate, Choice::Pick]);
        // 什么候选都没有：至少还有重建和选择
        assert_eq!(choices("/gone", None, None, &dirs(&[])), vec![Choice::Recreate, Choice::Pick]);
    }

    #[test]
    fn ancestor_search_never_offers_the_filesystem_root() {
        let got = choices("/gone/x", None, None, &dirs(&["/"]));
        assert_eq!(got, vec![Choice::Recreate, Choice::Pick]);
    }

    #[test]
    fn arrow_keys_wrap_around() {
        assert_eq!(step(0, 4, 1), 1);
        assert_eq!(step(3, 4, 1), 0, "到底回顶");
        assert_eq!(step(0, 4, -1), 3, "到顶回底");
        assert_eq!(step(0, 0, 1), 0, "空列表不越界");
    }
}
