//! PTY 启动前的纯逻辑：cwd 展开 / 回退、cwd 闸、resume 会话 id、会话起始目录校正。
//! spawn 本身两边各自实现（Tauri 用 portable-pty，GPUI 用 alacritty tty）。

use std::path::{Path, PathBuf};

// cwd 不存在时（session 自己 mv 走目录、外部删目录），向上找最近存在的祖先
// 找不到则回退 home，再不行 "/"——保证 shell 能起来
pub fn expand_tilde(cwd: &str) -> String {
    if cwd == "~" {
        return dirs::home_dir()
            .map(|h| h.to_string_lossy().into_owned())
            .unwrap_or_else(|| "/".to_string());
    }
    if let Some(rest) = cwd.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest).to_string_lossy().into_owned();
        }
    }
    cwd.to_string()
}

pub fn resolve_existing_cwd(cwd: &str) -> String {
    let expanded = expand_tilde(cwd);
    if Path::new(&expanded).is_dir() {
        return expanded;
    }
    let mut p = PathBuf::from(&expanded);
    while p.pop() {
        if p.is_dir() {
            return p.to_string_lossy().into_owned();
        }
    }
    dirs::home_dir()
        .map(|h| h.to_string_lossy().into_owned())
        .unwrap_or_else(|| "/".to_string())
}

/// 这个 pane 该不该拒绝启动。收成一个纯谓词只为了能测 —— `pty_spawn` 本体要
/// `Window` + `State`，测试里造不出来（和 `kill_tree` 从 `kill_pty` 里
/// 抽出来是同一个理由）。
pub fn must_refuse_cwd(cwd: &str, allow_fallback: Option<bool>) -> bool {
    allow_fallback == Some(false) && resolve_existing_cwd(cwd) != expand_tilde(cwd)
}

/// `clear && claude -r <id>`（`workspace-types.ts` 的 resumeInitCommand）里的会话 id。
/// id 会拼进文件路径，所以只认 uuid 形状（36 位 hex 和 `-`）。
pub fn resume_session_id(init_command: &str) -> Option<&str> {
    let id = init_command.split("claude -r ").nth(1)?.split_whitespace().next()?;
    let uuid_shaped = id.len() == 36 && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
    uuid_shaped.then_some(id)
}

/// 会话的起始目录：第一条主线（非 sidechain）用户记录的 cwd。后面的记录会被「在别的目录
/// `claude -r`」改写，不能用。
pub fn first_user_cwd(reader: impl std::io::BufRead) -> Option<String> {
    for line in reader.lines().map_while(Result::ok) {
        let Ok(rec) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
        if rec.get("type").and_then(|v| v.as_str()) != Some("user") { continue; }
        if rec.get("isSidechain").and_then(|v| v.as_bool()) == Some(true) { continue; }
        if let Some(c) = rec.get("cwd").and_then(|v| v.as_str()).filter(|c| !c.is_empty()) {
            return Some(c.to_string());
        }
    }
    None
}

/// resume tab 要不要改到会话的起始目录启动：起始目录**还在**且和请求的不同才改。
/// 起始目录不在了 = 项目被移走过（用户可能已用「指到新位置」恢复），这时 tab 记的新位置才是对的，
/// 硬改回去会和 cwd-missing 恢复流程互相打架。
pub fn corrected_resume_cwd(requested: &str, home: Option<String>) -> Option<String> {
    home.filter(|h| Path::new(h).is_dir() && *h != expand_tilde(requested))
}

/// 在 `~/.claude/projects/*/<id>.jsonl` 里找这个会话，返回它的起始目录。
pub fn session_home_cwd(projects_dir: &Path, session_id: &str) -> Option<String> {
    let file = std::fs::read_dir(projects_dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.path().join(format!("{session_id}.jsonl")))
        .find(|p| p.is_file())?;
    first_user_cwd(std::io::BufReader::new(std::fs::File::open(file).ok()?))
}

/// resume tab 的 cwd 闸（#173）。
///
/// 这条不变量承担的是：**目录没了的 resume tab 必须启动失败，而不是换个目录悄悄起来**。
/// 换 cwd 就是换钥匙 —— claude 只会去 `projects/<encode(realpath(cwd))>/<id>.jsonl`
/// 找会话，降级到祖先目录之后它报的是「No conversation found」，把一个能修的问题
/// 伪装成一个没救的问题。
#[cfg(test)]
mod cwd_gate_tests {
    use super::{expand_tilde, must_refuse_cwd, resolve_existing_cwd};

    #[test]
    fn refuses_only_when_fallback_is_forbidden_and_dir_is_gone() {
        let gone = "/definitely/not/a/path/makit-cwd-gate";
        // 前提：这个路径确实会触发降级，否则下面几条断言什么都没测到
        assert_ne!(resolve_existing_cwd(gone), expand_tilde(gone));

        // resume tab：必须拒
        assert!(must_refuse_cwd(gone, Some(false)));

        // shell / new tab：降级是对的（目的就是弄起一个 shell，在哪儿都行）
        assert!(!must_refuse_cwd(gone, Some(true)));
        // 老调用方不传这个参数 → None → 保持原有降级行为，向后兼容
        assert!(!must_refuse_cwd(gone, None));
    }

    #[test]
    fn never_refuses_when_the_dir_is_actually_there() {
        let here = std::env::temp_dir().to_string_lossy().into_owned();
        assert!(!must_refuse_cwd(&here, Some(false)), "目录在就不该拦");
    }
}

/// resume tab 必须在会话自己的起始目录启动（#190）。
///
/// 起因：在 shell 里手打 `claude -r` 后 tab 就地升级成 resume tab，但 tab 记的 cwd 还是那个 shell 的
/// 目录；新开终端又继承当前 tab 的 cwd —— 一个错目录会传给所有 tab。下次启动在错目录里
/// `claude -r`，claude 就在那个仓库里继续这个会话（记录里的 cwd、git 分支、Bash 默认目录全变）。
/// 会话的起始目录 = jsonl 第一条用户记录的 cwd = claude 存它的目录，是唯一可信的来源。
#[cfg(test)]
mod resume_cwd_tests {
    use super::{corrected_resume_cwd, first_user_cwd, resume_session_id, session_home_cwd};
    use std::io::Cursor;

    const SID: &str = "73ec5479-5b96-494c-919c-1f36a7e192fd";

    #[test]
    fn parses_session_id_from_resume_init_command() {
        assert_eq!(resume_session_id(&format!("clear && claude -r {SID}")), Some(SID));
        assert_eq!(resume_session_id(&format!("claude -r {SID}")), Some(SID));
        // codex 的会话不在 ~/.claude/projects 里，不管
        assert_eq!(resume_session_id(&format!("codex resume {SID}")), None);
        assert_eq!(resume_session_id("clear && claude"), None);
        assert_eq!(resume_session_id("zsh"), None);
        // id 会拼进文件路径：不是 uuid 形状的一律不认，杜绝 ../ 之类
        assert_eq!(resume_session_id("claude -r ../../etc/passwd"), None);
        assert_eq!(resume_session_id("claude -r 73ec5479"), None);
    }

    #[test]
    fn first_user_cwd_skips_non_user_and_sidechain_records() {
        let jsonl = [
            r#"{"type":"summary","summary":"x"}"#,
            r#"{"type":"user","isSidechain":true,"cwd":"/tmp/sidechain"}"#,
            r#"not json"#,
            r#"{"type":"user","cwd":""}"#,
            r#"{"type":"user","cwd":"/Users/me/RustProjects/makit"}"#,
            r#"{"type":"user","cwd":"/Users/me/other-project"}"#,
        ]
        .join("\n");
        assert_eq!(
            first_user_cwd(Cursor::new(jsonl)).as_deref(),
            Some("/Users/me/RustProjects/makit"),
            "起始目录是第一条有 cwd 的主线用户记录，后面被改写的不算"
        );
        assert_eq!(first_user_cwd(Cursor::new(r#"{"type":"summary"}"#)), None);
    }

    #[test]
    fn corrects_only_to_an_existing_home_that_differs() {
        let home = std::env::temp_dir().to_string_lossy().trim_end_matches('/').to_string();
        // 在别的目录被恢复 → 改回起始目录
        assert_eq!(corrected_resume_cwd("/Users/me/other-project", Some(home.clone())), Some(home.clone()));
        // 本来就在起始目录 → 不改（不通知前端）
        assert_eq!(corrected_resume_cwd(&home, Some(home.clone())), None);
        // 起始目录已经不在了（目录被移走、用户用「指到新位置」恢复过）→ 不改，
        // 否则会和 cwd-missing 恢复流程互相打架：改回去 → 被拒 → relink 到新目录 → 又被改回去
        assert_eq!(corrected_resume_cwd("/Users/me/new-place", Some("/definitely/not/a/path/makit-home".into())), None);
        // 找不到会话记录 → 不改
        assert_eq!(corrected_resume_cwd("/Users/me/x", None), None);
    }

    #[test]
    fn finds_session_file_under_any_project_folder() {
        let root = std::env::temp_dir().join(format!("makit-resume-cwd-{}", std::process::id()));
        let proj = root.join("-Users-me-RustProjects-makit");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::create_dir_all(root.join("-Users-me-other")).unwrap();
        std::fs::write(
            proj.join(format!("{SID}.jsonl")),
            r#"{"type":"user","cwd":"/Users/me/RustProjects/makit"}"#,
        )
        .unwrap();

        assert_eq!(session_home_cwd(&root, SID).as_deref(), Some("/Users/me/RustProjects/makit"));
        assert_eq!(session_home_cwd(&root, "00000000-0000-0000-0000-000000000000"), None);
        assert_eq!(session_home_cwd(&root.join("nope"), SID), None, "projects 目录不存在也不 panic");
        std::fs::remove_dir_all(&root).ok();
    }
}
