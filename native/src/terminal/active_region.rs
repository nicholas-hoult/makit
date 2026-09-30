//! 活动区识别（#231 第 3.0 步）：在 claude（内联模式）的终端画面里，找出「活动区」——输入框、转圈状态、
//! 权限确认 / 斜杠菜单这一块。混合布局只画这几行，上面的历史由阅读视图从会话文件画（#231 TRD §15）。
//!
//! 为什么看**画面**而不是解析转义序列：claude 用 Ink 做差分重画（只改变化的格子，每帧光标都停回输入框），
//! 转义序列里看不出「动态区有几行」；而画出来的界面结构是稳定的——实测 claude 2.1.285：
//! - 空闲 / 转圈：输入框被两条满宽 `─` 横线夹住，中间一行以 `❯` 开头，下面还有一两行状态（`⏸ manual mode on …`）；
//!   转圈时横线上方还有一行 `· Musing…` / `✽ Misting… (3s · ↓ 214 tokens)`，完成后是 `✻ Worked for 1s`
//! - 权限确认：输入框消失，菜单从一条满宽横线下面一直画到底（`❯ 1. Yes` / `2. …` / `3. No`）
//! - 斜杠菜单：输入框还在，菜单画在**输入框下面**
//!
//! 样本在 `tests/fixtures/active-region/`（真实 claude 画面，路径 / 会话 id 已脱敏）。
//! 为什么单独测：识别错了在界面上就是「菜单选项被截掉」「转圈那行在历史和活动区之间闪」「输入框不见了」。

/// 活动区在画面里的行范围：`start..=end`（0 起，含两端）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Region {
    pub start: usize,
    pub end: usize,
}

impl Region {
    pub fn rows(&self) -> usize {
        self.end + 1 - self.start
    }
}

/// 满宽横线：至少这么多个连续 `─`（claude 按终端宽度画，窄到 30 列也远超这个数）
const RULE_MIN: usize = 20;

/// 在画面（每行一段文字，行尾空白已去掉）里找活动区。认不出 → None，调用方退回纯终端
pub fn find(lines: &[&str]) -> Option<Region> {
    if let Some(r) = find_codex(lines) {
        return Some(r);
    }
    let end = lines.iter().rposition(|l| !l.trim().is_empty())?;
    let is_rule = |i: usize| is_rule_line(lines[i]);
    // 1. 输入框：「横线 / ❯ 行 / 横线」，取最后一组（斜杠菜单画在它下面，也算活动区）
    let prompt_top = (1..end).rev().find(|&i| lines[i].trim_start().starts_with('❯') && is_rule(i - 1) && i + 1 <= end && is_rule(i + 1)).map(|i| i - 1);
    // 2. 没有输入框：权限确认等菜单把输入框换掉了，从最后一条满宽横线开始
    let top = match prompt_top {
        Some(t) => t,
        None => {
            let t = (0..=end).rev().find(|&i| is_rule(i))?;
            // 横线下面要有像菜单的内容（❯ 选中项或编号选项），否则只是输出里的一条分隔线
            let menu_like = lines[t + 1..=end].iter().any(|l| {
                let l = l.trim_start();
                l.starts_with('❯') || l.starts_with("1.") || l.contains("Esc to cancel")
            });
            if !menu_like {
                return None;
            }
            t
        }
    };
    // 3. 往上带上紧挨着的状态行（转圈 / Worked for），中间最多隔一行空行；不然这行会在历史和活动区之间闪
    let mut start = top;
    let mut i = top;
    let mut blanks = 0;
    while i > 0 {
        i -= 1;
        let l = lines[i].trim();
        if l.is_empty() {
            blanks += 1;
            if blanks > 1 {
                break;
            }
            continue;
        }
        if is_status_line(l) {
            start = i;
        }
        break;
    }
    Some(Region { start, end })
}

/// codex 0.40：输入框是 `▌` 开头的行，下面隔一行空行是提示 `⏎ send   ⌃J newline …`；工作中时上面还有
/// `  Working (1s • Esc to interrupt)`。历史里用户发的消息**也是** `▌` 开头，所以必须以提示行为锚点往上找
fn find_codex(lines: &[&str]) -> Option<Region> {
    let Some(hint) = lines.iter().rposition(|l| l.trim_start().starts_with("⏎ send")) else {
        return find_codex_menu(lines);
    };
    let prompt = (hint.saturating_sub(3)..hint).rev().find(|&i| lines[i].starts_with('▌'))?;
    let mut start = prompt;
    // 往上带上 Working 状态行（中间最多隔一行空行）
    for i in (prompt.saturating_sub(2)..prompt).rev() {
        let l = lines[i].trim();
        if l.starts_with("Working (") {
            start = i;
            break;
        }
        if !l.is_empty() {
            break;
        }
    }
    let end = lines.iter().rposition(|l| !l.trim().is_empty()).filter(|&e| e >= hint).unwrap_or(hint);
    Some(Region { start, end })
}

/// codex 审批菜单：输入框和 `⏎ send` 提示都没了，屏底是一组连续的 `▌` 行
/// （`▌Allow command?` / `▌ Yes   Always   No, provide feedback` / `▌ Approve and run the command`）。
/// 上面的 `• Proposed Command` 属于历史（codex 把函数调用写进会话文件）
fn find_codex_menu(lines: &[&str]) -> Option<Region> {
    let end = lines.iter().rposition(|l| !l.trim().is_empty())?;
    if !lines[end].starts_with('▌') {
        return None;
    }
    let mut start = end;
    while start > 0 && lines[start - 1].starts_with('▌') {
        start -= 1;
    }
    // 至少两行、而且像选项：只有一行 ▌ 可能只是历史里的用户消息
    let body = &lines[start..=end];
    let looks_like_menu = body.len() >= 2 && body.iter().any(|l| l.contains("Yes") || l.contains("Allow") || l.contains("Approve"));
    looks_like_menu.then_some(Region { start, end })
}

fn is_rule_line(l: &str) -> bool {
    let t = l.trim();
    t.chars().count() >= RULE_MIN && t.chars().all(|c| c == '─')
}

/// claude 的状态行：转圈符号开头（· ✢ ✳ ✶ ✻ ✽ ∗ 等，随帧变化），后面是动词 / 「Worked for」
fn is_status_line(l: &str) -> bool {
    let mut cs = l.chars();
    let (Some(first), Some(second)) = (cs.next(), cs.next()) else { return false };
    matches!(first, '·' | '✢' | '✳' | '✶' | '✻' | '✽' | '∗' | '*' | '⏺') && second == ' ' && first != '⏺'
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!("{}/tests/fixtures/active-region/{name}.txt", env!("CARGO_MANIFEST_DIR"))).unwrap()
    }
    fn region(name: &str) -> (Option<Region>, Vec<String>) {
        let text = fixture(name);
        let lines: Vec<String> = text.lines().map(|l| l.to_string()).collect();
        let refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
        (find(&refs), lines)
    }

    #[test]
    fn idle_prompt_includes_the_done_status_line_and_the_mode_line() {
        let (r, lines) = region("idle-prompt");
        let r = r.expect("空闲时有输入框");
        assert!(lines[r.start].starts_with("✻ Worked for"), "从「Worked for」那行开始：{:?}", lines[r.start]);
        assert!(lines[r.end].contains("manual mode on"), "到模式提示那行结束：{:?}", lines[r.end]);
        assert!(lines[r.start..=r.end].iter().any(|l| l.starts_with('❯')), "输入框在里面");
        assert!(!lines[r.start..=r.end].iter().any(|l| l.starts_with("⏺ ok")), "回答本身属于历史，不在活动区");
    }

    #[test]
    fn thinking_includes_the_spinner_line_so_it_does_not_flicker_between_history_and_active() {
        let (r, lines) = region("thinking");
        let r = r.expect("转圈时有输入框");
        assert!(lines[r.start].contains("Musing"), "转圈那行在活动区里：{:?}", lines[r.start]);
        assert!(!lines[r.start..=r.end].iter().any(|l| l.contains("请用 Write")), "用户刚发的消息属于历史");
    }

    #[test]
    fn permission_menu_keeps_every_option() {
        let (r, lines) = region("perm-menu");
        let r = r.expect("权限菜单");
        let body = lines[r.start..=r.end].join("\n");
        for want in ["Do you want to create a.txt?", "1. Yes", "2. Yes, and switch", "3. No", "Esc to cancel"] {
            assert!(body.contains(want), "少了「{want}」——菜单选项被截掉，用户就选不了：\n{body}");
        }
        assert!(body.contains("Create file"), "菜单连同文件预览一起在活动区");
        assert!(!body.contains("⏺ Write(a.txt)"), "工具调用那行属于历史");
    }

    #[test]
    fn slash_menu_below_the_prompt_is_part_of_the_active_region() {
        let (r, lines) = region("slash-menu");
        let r = r.expect("斜杠菜单");
        let body = lines[r.start..=r.end].join("\n");
        // claude 在 ❯ 后面用的是不换行空格 U+00A0，不是普通空格
        assert!(lines[r.start..=r.end].iter().any(|l| l.starts_with("❯\u{a0}/")), "输入框在里面");
        assert!(body.contains("/loop") && body.contains("/btw"), "菜单全部在活动区");
        assert!(!body.contains("Claude Code v"), "欢迎横幅不在活动区");
    }

    #[test]
    fn long_output_pushes_the_prompt_to_the_bottom_and_history_stays_out() {
        let (r, lines) = region("long-scrolled");
        let r = r.expect("长输出后仍有输入框");
        assert_eq!(r.end, lines.len() - 1, "贴着屏底");
        assert!(lines[r.start].contains("Misting"), "从转圈那行开始");
        assert!(r.rows() <= 8, "活动区只有几行，历史（1..60）不能被当成活动区：{} 行", r.rows());
    }

    #[test]
    fn the_prompt_uses_a_no_break_space_after_the_chevron() {
        // 真实样本里的事实：以后要按「❯ 后面跟的内容」判断时别用普通空格去匹配
        let text = fixture("slash-menu");
        assert!(text.contains("❯\u{a0}/"), "claude 2.1.285 的输入框是 ❯ + U+00A0");
        assert!(!text.contains("❯ /"));
    }


    // ---- codex 0.40（画面结构和 claude 不同：输入框 `▌` 开头，下面隔一行是 `⏎ send …` 提示）----

    #[test]
    fn codex_idle_prompt_and_its_hint_line() {
        let (r, lines) = region("codex-idle");
        let r = r.expect("codex 空闲时有输入框");
        assert!(lines[r.start].starts_with('▌'), "从输入框开始：{:?}", lines[r.start]);
        assert!(lines[r.end].contains("⏎ send"), "到提示行结束：{:?}", lines[r.end]);
        assert!(!lines[r.start..=r.end].iter().any(|l| l.contains("OpenAI Codex")), "欢迎框不在活动区");
    }

    #[test]
    fn codex_working_includes_the_status_but_not_the_users_message_above() {
        let (r, lines) = region("codex-working");
        let r = r.expect("codex 工作中");
        let body = lines[r.start..=r.end].join("\n");
        assert!(lines[r.start].contains("Working ("), "带上 Working 状态行：{:?}", lines[r.start]);
        assert!(body.contains("⏎ send"));
        // 历史里用户发的消息也是 ▌ 开头，不能被当成输入框
        assert!(!body.contains("只回复两个字母"), "用户消息属于历史：\n{body}");
    }


    #[test]
    fn codex_after_an_answer_the_answer_is_history() {
        let (r, lines) = region("codex-done");
        let r = r.expect("回答后回到输入框");
        let body = lines[r.start..=r.end].join("\n");
        assert!(!body.contains("> ok"), "codex 的回答属于历史：\n{body}");
        assert!(body.contains("⏎ send"));
    }

    #[test]
    fn codex_long_output_keeps_the_region_small() {
        let (r, lines) = region("codex-long");
        let r = r.expect("长输出后仍有输入框");
        assert_eq!(r.end, lines.len() - 1, "贴着屏底");
        assert!(r.rows() <= 5, "历史（1..60）不能被当成活动区：{} 行", r.rows());
    }

    #[test]
    fn codex_approval_menu_replaces_the_prompt_and_keeps_every_choice() {
        let (r, lines) = region("codex-perm-menu");
        let r = r.expect("codex 审批菜单");
        let body = lines[r.start..=r.end].join("\n");
        for want in ["Allow command?", "Yes", "Always", "No, provide feedback", "Approve and run"] {
            assert!(body.contains(want), "少了「{want}」：\n{body}");
        }
        assert!(!body.contains("请运行 shell 命令"), "用户消息属于历史");
        assert!(!body.contains("我将运行"), "codex 的说明属于历史");
    }

    #[test]
    fn nothing_recognisable_means_unknown() {
        assert_eq!(find(&["$ ls", "a.txt  b.txt", "$"]), None, "普通 shell：不认识，退回纯终端");
        assert_eq!(find(&[]), None);
        assert_eq!(find(&["", "", ""]), None);
    }

    #[test]
    fn a_short_dash_run_is_not_a_rule() {
        // 输出里出现的 `---` 或几个 `─` 不是输入框的横线
        assert_eq!(find(&["表头", "─────", "❯", "─────"]), None);
    }
}
