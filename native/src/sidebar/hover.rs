//! 悬停详情卡的纯逻辑（SessionTree.tsx 的 hoverCard 部分）：出不出、延迟多久、放在哪、显示哪几行。
//!
//! 为什么单独测：卡片里每行都是「条件出现」的（当前 cwd 只在和启动 cwd 不同时出现、子进程要滤掉
//! shell / claude 自己……），条件写错在界面上是「多了一行重复信息」或「该有的 PID 没了」，
//! 而卡片要悬停 400ms 才出来，肉眼回归很难覆盖全。

use makit_core::SessionMeta;

/// 悬停多久出卡片：`makit-hover-mode` = always（400ms）/ cmd（按住 ⌘ 才出，立刻）/ off。None = 不出
pub fn hover_delay_ms(mode: &str, cmd_pressed: bool) -> Option<u64> {
    match mode {
        "off" => None,
        "cmd" if !cmd_pressed => None,
        "cmd" => Some(0),
        _ => Some(400),
    }
}

/// 鼠标离开行 / 卡片后多久关
pub const HOVER_CLOSE_MS: u64 = 200;

/// 卡片左上角：行的右边 +4；y 取 min(行 top, 窗口高 - 300)
pub fn card_origin(row_right: f32, row_top: f32, window_h: f32) -> (f32, f32) {
    (row_right + 4.0, row_top.min(window_h - 300.0))
}

/// 卡片标题（同 TS：display_name → first_user_msg → [short_id]，不压平，显示时再压）
pub fn card_title(s: &SessionMeta) -> String {
    if !s.display_name.is_empty() {
        s.display_name.clone()
    } else if !s.first_user_msg.is_empty() {
        s.first_user_msg.clone()
    } else {
        format!("[{}]", s.short_id)
    }
}

/// 命令的第一个词取 basename：`/usr/local/bin/node server.js` → `node`
fn command_head(cmd: &str) -> &str {
    let first = cmd.split_whitespace().next().unwrap_or("");
    first.rsplit('/').next().unwrap_or("")
}

fn clip(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// 卡片的每一行（标签, 值）。点一行复制它的值
pub fn card_rows(s: &SessionMeta) -> Vec<(&'static str, String)> {
    const HIDDEN: [&str; 6] = ["sh", "bash", "zsh", "ps", "claude", "caffeinate"];
    let procs: Vec<String> = s
        .child_processes
        .iter()
        .filter(|p| !HIDDEN.contains(&command_head(&p.command)))
        .map(|p| format!("{}({})", command_head(&p.command), p.pid))
        .collect();
    let cwd_same = s.last_cwd.is_empty() || s.last_cwd == s.cwd;
    let mut rows = vec![("ID", s.session_id.clone()), ("启动 cwd", s.cwd.clone())];
    if !cwd_same {
        rows.push(("当前 cwd", s.last_cwd.clone()));
    }
    if !s.git_root.is_empty() && s.git_root != s.cwd {
        rows.push(("项目", s.git_root.clone()));
    }
    if !s.git_branch.is_empty() {
        rows.push(("分支", s.git_branch.clone()));
    }
    rows.push(("消息", format!("{} 条", s.user_msg_count)));
    if !s.first_user_msg.is_empty() {
        rows.push(("首话题", clip(&s.first_user_msg, 80)));
    }
    if !s.last_user_msg.is_empty() && s.last_user_msg != s.first_user_msg {
        rows.push(("末话题", clip(&s.last_user_msg, 80)));
    }
    if s.running {
        rows.push(("PID", s.pid.to_string()));
    }
    if !procs.is_empty() {
        rows.push(("子进程", procs.join(", ")));
    }
    rows.push(("时间", s.mtime_display.clone()));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::sessions::tests::session;
    use makit_core::ProcessInfo;

    #[test]
    fn delay_by_mode() {
        assert_eq!(hover_delay_ms("always", false), Some(400));
        assert_eq!(hover_delay_ms("always", true), Some(400));
        assert_eq!(hover_delay_ms("cmd", true), Some(0));
        assert_eq!(hover_delay_ms("cmd", false), None, "cmd 模式没按 ⌘ 不出");
        assert_eq!(hover_delay_ms("off", true), None);
        assert_eq!(hover_delay_ms("", false), Some(400), "没存过按 always");
    }

    #[test]
    fn origin_right_of_row_and_kept_on_screen() {
        assert_eq!(card_origin(280.0, 100.0, 900.0), (284.0, 100.0));
        assert_eq!(card_origin(280.0, 800.0, 900.0), (284.0, 600.0), "靠近窗口底往上挪");
    }

    #[test]
    fn minimal_rows() {
        let s = SessionMeta { mtime_display: "2026-09-27 10:00".into(), last_cwd: String::new(), first_user_msg: String::new(), ..session("abc") };
        let rows = card_rows(&s);
        let labels: Vec<&str> = rows.iter().map(|r| r.0).collect();
        assert_eq!(labels, ["ID", "启动 cwd", "消息", "时间"], "cwd 相同、没分支、没话题、没在跑");
        assert_eq!(rows[2].1, "1 条");
        assert_eq!(card_title(&s), "[abc]");
    }

    #[test]
    fn conditional_rows() {
        let long: String = "长".repeat(100);
        let s = SessionMeta {
            cwd: "/p".into(),
            last_cwd: "/p/sub".into(),
            git_root: "/repo".into(),
            git_branch: "main".into(),
            first_user_msg: long.clone(),
            last_user_msg: "最后".into(),
            running: true,
            pid: 42,
            child_processes: vec![
                ProcessInfo { pid: 1, ppid: 0, command: "/bin/zsh -l".into() },
                ProcessInfo { pid: 2, ppid: 0, command: "/usr/local/bin/node server.js".into() },
                ProcessInfo { pid: 3, ppid: 0, command: "claude".into() },
                ProcessInfo { pid: 4, ppid: 0, command: "caffeinate -i".into() },
                ProcessInfo { pid: 5, ppid: 0, command: "cargo test".into() },
            ],
            display_name: "改过名".into(),
            ..session("abc")
        };
        let rows = card_rows(&s);
        let labels: Vec<&str> = rows.iter().map(|r| r.0).collect();
        assert_eq!(labels, ["ID", "启动 cwd", "当前 cwd", "项目", "分支", "消息", "首话题", "末话题", "PID", "子进程", "时间"]);
        assert_eq!(rows[6].1.chars().count(), 80, "首话题截 80 字");
        assert_eq!(rows[8].1, "42");
        assert_eq!(rows[9].1, "node(2), cargo(5)", "滤掉 sh/bash/zsh/ps/claude/caffeinate");
        assert_eq!(card_title(&s), "改过名");
        let same = SessionMeta { last_user_msg: long.clone(), first_user_msg: long, git_root: "/p".into(), ..s };
        let labels: Vec<&str> = card_rows(&same).iter().map(|r| r.0).collect();
        assert!(!labels.contains(&"末话题"), "末话题和首话题相同不重复");
        assert!(!labels.contains(&"项目"), "项目和 cwd 相同不重复");
    }
}
