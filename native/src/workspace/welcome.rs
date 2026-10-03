//! 欢迎卡（pane 里一个标签都没有时显示，`ContainerView.tsx` 的 `.welcome-card`）。
//!
//! 键位**读单一快捷键表**（`actions::keymap()`）：每一行写的是 GPUI 键位串，显示文字由 `key_label` 从键位串
//! 推出来，测试检查每个键都真的在表里 —— 改了快捷键忘了改欢迎卡，测试会红（修 #224「两份快捷键表」）。
//! 说明文字沿用 Tauri 版欢迎卡的措辞（比表里的 desc 更口语，且按「轴」分组在教规则），不从表里取。
//! 分组、顺序、键帽的拆法（修饰键和方向键分两个键帽）照搬 TS 版。

use gpui::{div, prelude::*, px, AnyElement, ClickEvent, FontWeight, Window};

use crate::theme::Theme;

/// 欢迎卡副标题（#256 A3）。点名两个工具：只用 Codex 的人看到「Claude Code Session 管理」会觉得不是给自己的。
/// 做多语言（#71）时和别的文案一起搬走
pub const TAGLINE: &str = "Claude Code · Codex 会话管理 · 工作区终端";

/// 欢迎卡底部的信任说明（#256 A5）。**写进去的每句话都要是真的**（核实见 TRD #256 §2.1）：
/// makit 读取这两个目录；自己的数据在 `~/.claude/makit`；不会把会话内容发出去。
/// 不写「只读」——恢复搬走了目录的会话时会在 `~/.claude/projects` 里建软链，点了才装的 hook 会写 `settings.json`；
/// 也不写「不联网」——用户明确不要（#200 的版本检查之后可能联网）
pub const TRUST_NOTE: &str = "makit 在本机读取你的会话记录（~/.claude、~/.codex），不会上传；自己的设置存在 ~/.claude/makit。";

/// 新用户的「3 步开始」（#256 A4）：欢迎卡信息量太大——没有会话时一屏 4 组 20 多条快捷键会把新用户淹没。
/// 没有会话时换成这 3 步，完整快捷键表折叠在「查看全部快捷键」后面；已经有会话（只是关掉了这个 pane 的标签）时
/// 不折叠，直接显示完整表——这是熟手在用的路径，不该被「新手引导」挡住
pub const STEPS: &[&str] = &["在终端里运行 claude 或 codex，开始第一个会话", "会话自动出现在左侧侧栏", "点它可以随时恢复对话"];

/// 没有会话时要不要显示完整快捷键表：默认折叠（`expanded == false`）；用户点过「查看全部快捷键」才展开。
/// 已经有会话时（`has_hint == false`，#197 的 `empty_hint` 判定）不折叠，`expanded` 不起作用
pub fn show_full_shortcuts(has_hint: bool, expanded: bool) -> bool {
    !has_hint || expanded
}

/// 一行左边的键帽怎么排
#[derive(Clone, Copy, Debug)]
pub enum Keys {
    /// 一个键帽
    One(&'static str),
    /// 两个键帽并排（⌘[ ⌘]）
    Pair(&'static str, &'static str),
    /// 区间：键帽 – 键帽（⌘1 – ⌘9）
    Range(&'static str, &'static str),
    /// 修饰键 + 四个方向键分两个键帽（四个箭头挤一个键帽里 ← → 会连成一根长箭头）。参数是修饰键前缀，如 "alt-cmd"
    ModArrows(&'static str),
    /// 还没进快捷键表的组件内键位（D 浮层包的面板 ↑↓ / ⌘↩ / Esc），原样显示。进表后改成上面几种
    Literal(&'static [&'static str]),
}

pub struct Row {
    pub keys: Keys,
    pub desc: &'static str,
}

pub struct Group {
    pub title: &'static str,
    pub rows: &'static [Row],
}

const fn r(keys: Keys, desc: &'static str) -> Row {
    Row { keys, desc }
}

pub const GROUPS: &[Group] = &[
    Group {
        title: "搜索 · 命令",
        rows: &[
            r(Keys::One("cmd-k"), "全局搜索（命令面板）"),
            r(Keys::One("cmd-f"), "当前终端内搜索"),
            r(Keys::One("cmd-shift-f"), "侧栏 session 搜索"),
            r(Keys::One("cmd-i"), "通知中心"),
            r(Keys::One("cmd-r"), "刷新 session 列表"),
        ],
    },
    Group {
        title: "Tab · ⌘ 轴",
        rows: &[
            r(Keys::One("cmd-t"), "新建 shell tab"),
            r(Keys::One("cmd-w"), "关闭当前 tab"),
            r(Keys::ModArrows("cmd"), "上 / 下一个 tab"),
            r(Keys::Pair("cmd-[", "cmd-]"), "同上（浏览器习惯）"),
            r(Keys::Pair("ctrl-shift-tab", "ctrl-tab"), "同上（VS Code 习惯）"),
            r(Keys::Range("cmd-1", "cmd-9"), "第 N 个 tab"),
        ],
    },
    Group {
        title: "Pane · ⌥⌘ 轴",
        rows: &[
            r(Keys::One("cmd-d"), "左右分屏"),
            r(Keys::One("cmd-shift-d"), "上下分屏"),
            r(Keys::ModArrows("alt-cmd"), "切到相邻 pane"),
            r(Keys::Range("alt-cmd-1", "alt-cmd-9"), "第 N 个 pane"),
            r(Keys::One("alt-cmd-enter"), "最大化 / 还原 pane"),
        ],
    },
    Group {
        title: "侧栏 · 面板",
        rows: &[
            r(Keys::One("cmd-b"), "折叠项目列表"),
            r(Keys::One("cmd-l"), "在侧栏定位当前 session"),
            r(Keys::Literal(&["↑", "↓"]), "面板内上下选择"),
            r(Keys::Literal(&["⌘↩"]), "命令面板：在 split 打开"),
            r(Keys::Literal(&["Esc"]), "关闭面板 / 取消"),
        ],
    },
];

/// GPUI 键位串 → 键帽上的字：`alt-cmd-1` → `⌥⌘1`，`cmd-shift-d` → `⌘⇧D`，`ctrl-shift-tab` → `⌃⇧Tab`。
/// 修饰键按键位串里写的顺序排（和 Tauri 版欢迎卡的写法一致）
pub fn key_label(keys: &str) -> String {
    let mut out = String::new();
    let parts: Vec<&str> = keys.split('-').collect();
    // 「cmd--」这种键本身是 "-" 的，split 出来末尾是两个空串
    let (mods, key) = if keys.ends_with("--") { (&parts[..parts.len() - 2], "-") } else { (&parts[..parts.len() - 1], parts[parts.len() - 1]) };
    for m in mods {
        out.push_str(match *m {
            "cmd" => "⌘",
            "alt" => "⌥",
            "shift" => "⇧",
            "ctrl" => "⌃",
            other => other,
        });
    }
    out.push_str(&match key {
        "enter" => "↩".to_string(),
        "tab" => "Tab".to_string(),
        "escape" => "Esc".to_string(),
        "left" => "←".to_string(),
        "right" => "→".to_string(),
        "up" => "↑".to_string(),
        "down" => "↓".to_string(),
        k if k.chars().count() == 1 => k.to_uppercase(),
        k => k.to_string(),
    });
    out
}

/// 这一行引用的全部键位串（测试用它核对快捷键表）
pub fn referenced_keys(keys: Keys) -> Vec<String> {
    match keys {
        Keys::One(a) => vec![a.into()],
        Keys::Pair(a, b) | Keys::Range(a, b) => vec![a.into(), b.into()],
        Keys::ModArrows(m) => ["left", "right", "up", "down"].iter().map(|d| format!("{m}-{d}")).collect(),
        Keys::Literal(_) => vec![],
    }
}

/// 一行要画的键帽（`None` = 区间号 –）
pub fn keycaps(keys: Keys) -> Vec<Option<String>> {
    match keys {
        Keys::One(a) => vec![Some(key_label(a))],
        Keys::Pair(a, b) => vec![Some(key_label(a)), Some(key_label(b))],
        Keys::Range(a, b) => vec![Some(key_label(a)), None, Some(key_label(b))],
        Keys::ModArrows(m) => vec![Some(key_label(&format!("{m}-x")).trim_end_matches('X').to_string()), Some("← → ↑ ↓".into())],
        Keys::Literal(ks) => ks.iter().map(|k| Some(k.to_string())).collect(),
    }
}

/// 画欢迎卡（App.css `.container-empty` / `.welcome-*` 的数值）。
/// `shortcuts_expanded` / `on_toggle_shortcuts`：新用户（`hint` 有值）默认折叠完整快捷键表，点「查看全部快捷键」才展开（#256 A4）
pub fn render_welcome(
    t: &Theme,
    hint: Option<super::empty_hint::EmptyHint>,
    shortcuts_expanded: bool,
    on_toggle_shortcuts: impl Fn(&ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> AnyElement {
    let kbd = |text: String| {
        div()
            .flex_none()
            .font_family(".AppleSystemUIFontMonospaced") // = CSS ui-monospace（键帽 .welcome-key）
            .text_size(px(11.0))
            .px(px(6.0))
            .py(px(2.0))
            .bg(t.bg_soft)
            .border_1()
            .border_b_2()
            .border_color(t.border)
            .rounded(px(4.0))
            .text_color(t.fg)
            .child(text)
    };
    let has_hint = hint.is_some();
    let show_full = show_full_shortcuts(has_hint, shortcuts_expanded);
    let groups: Vec<AnyElement> = GROUPS
        .iter()
        .map(|g| {
            let rows: Vec<AnyElement> = g
                .rows
                .iter()
                .map(|row| {
                    let caps: Vec<AnyElement> = keycaps(row.keys)
                        .into_iter()
                        .map(|c| match c {
                            Some(text) => kbd(text).into_any_element(),
                            None => div().text_size(px(10.0)).opacity(0.45).child("–").into_any_element(),
                        })
                        .collect();
                    // 左列固定 108px、键帽右对齐：键帽宽度参差时右边说明文字仍有一条竖直基线
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.0))
                        .text_size(px(12.0))
                        .text_color(t.fg_muted)
                        .child(div().flex_none().w(px(108.0)).flex().items_center().justify_end().gap(px(3.0)).children(caps))
                        .child(div().flex_1().min_w_0().child(row.desc))
                        .into_any_element()
                })
                .collect();
            div()
                .flex()
                .flex_col()
                .gap(px(6.0))
                // 两列；pane 窄到放不下两列（≈ TS 的 700px 断点）时自动折成一列
                .flex_1()
                .min_w(px(300.0))
                .child(
                    div()
                        .mb(px(4.0))
                        .text_size(px(11.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(t.var("--accent-text"))
                        // CSS 的 text-transform: uppercase
                        .child(g.title.to_uppercase()),
                )
                .children(rows)
                .into_any_element()
        })
        .collect();
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .p(px(24.0))
        .text_color(t.fg_muted)
        .child(
            div()
                .max_w(px(720.0))
                .w_full()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(12.0))
                .child(div().text_size(px(28.0)).font_weight(FontWeight::SEMIBOLD).text_color(t.fg).child("makit"))
                .child(div().text_size(px(13.0)).opacity(0.8).child(TAGLINE))
                .child(
                    div()
                        .mt(px(4.0))
                        .text_size(px(12.0))
                        .opacity(0.7)
                        .text_center()
                        .child("点侧栏 session 卡片恢复对话；点项目 worktree 起新会话；⌘ 点击为纯 shell"),
                )
                .when(has_hint, |d| {
                    // 新用户「3 步开始」（#256 A4）：取代一屏 20 多条快捷键
                    d.child(
                        div()
                            .mt(px(16.0))
                            .flex()
                            .flex_col()
                            .gap(px(8.0))
                            .children(STEPS.iter().enumerate().map(|(i, step)| {
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(10.0))
                                    .text_size(px(13.0))
                                    .text_color(t.fg_muted)
                                    .child(
                                        div()
                                            .flex_none()
                                            .size(px(20.0))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .rounded(px(10.0))
                                            .bg(t.bg_soft)
                                            .text_size(px(11.0))
                                            .text_color(t.var("--accent-text"))
                                            .child((i + 1).to_string()),
                                    )
                                    .child(*step)
                                    .into_any_element()
                            })),
                    )
                })
                .when_some(hint, |d, h| {
                    // 没有会话时的提示（#197）：醒目但不抢戏——accent 色细边的卡片，标题 + 说明 + 检测结果
                    d.child(
                        div()
                            .mt(px(16.0))
                            .max_w(px(520.0))
                            .w_full()
                            .flex()
                            .flex_col()
                            .gap(px(6.0))
                            .px(px(16.0))
                            .py(px(12.0))
                            .rounded(px(8.0))
                            .border_1()
                            .border_color(t.var("--accent-text"))
                            .bg(t.bg_soft)
                            .child(div().text_size(px(14.0)).font_weight(FontWeight::SEMIBOLD).text_color(t.fg).child(h.title))
                            .children(h.lines.into_iter().map(|l| div().text_size(px(12.0)).line_height(px(18.0)).text_color(t.fg_muted).child(l)))
                            .child(div().mt(px(2.0)).text_size(px(11.0)).text_color(t.fg_subtle).child(h.status)),
                    )
                })
                .when(has_hint, |d| {
                    d.child(
                        div()
                            .id("toggle-shortcuts")
                            .mt(px(16.0))
                            .text_size(px(12.0))
                            .text_color(t.var("--accent-text"))
                            .cursor_pointer()
                            .on_click(on_toggle_shortcuts)
                            .child(if show_full { "收起快捷键" } else { "查看全部快捷键" }),
                    )
                })
                .when(show_full, |d| {
                    d.child(div().mt(px(24.0)).w_full().flex().flex_wrap().gap_x(px(24.0)).gap_y(px(16.0)).children(groups))
                })
                // 信任说明：最下面一行小字，不抢戏（#256 A5）
                .child(div().mt(px(20.0)).text_size(px(11.0)).text_color(t.fg_subtle).text_center().child(TRUST_NOTE)),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_label_matches_the_tauri_welcome_card() {
        assert_eq!(key_label("cmd-k"), "⌘K");
        assert_eq!(key_label("cmd-shift-f"), "⌘⇧F");
        assert_eq!(key_label("alt-cmd-1"), "⌥⌘1");
        assert_eq!(key_label("alt-cmd-enter"), "⌥⌘↩");
        assert_eq!(key_label("ctrl-shift-tab"), "⌃⇧Tab");
        assert_eq!(key_label("ctrl-tab"), "⌃Tab");
        assert_eq!(key_label("cmd-["), "⌘[");
        assert_eq!(key_label("cmd--"), "⌘-");
    }

    #[test]
    fn keycaps_split_like_tauri() {
        assert_eq!(keycaps(Keys::ModArrows("alt-cmd")), vec![Some("⌥⌘".into()), Some("← → ↑ ↓".into())]);
        assert_eq!(keycaps(Keys::ModArrows("cmd")), vec![Some("⌘".into()), Some("← → ↑ ↓".into())]);
        assert_eq!(keycaps(Keys::Range("cmd-1", "cmd-9")), vec![Some("⌘1".into()), None, Some("⌘9".into())]);
        assert_eq!(keycaps(Keys::Pair("ctrl-shift-tab", "ctrl-tab")), vec![Some("⌃⇧Tab".into()), Some("⌃Tab".into())]);
    }

    /// 欢迎卡上的每个键都必须真的绑在快捷键表里（单一数据源）
    #[test]
    fn every_key_on_the_card_is_in_the_keymap() {
        let km = crate::actions::keymap();
        for g in GROUPS {
            for row in g.rows {
                for k in referenced_keys(row.keys) {
                    assert!(km.iter().any(|s| s.keys == k), "欢迎卡「{}」里的 {k} 不在快捷键表里", row.desc);
                }
            }
        }
    }

    #[test]
    fn four_groups_like_tauri() {
        assert_eq!(GROUPS.len(), 4);
        assert_eq!(GROUPS.iter().map(|g| g.rows.len()).sum::<usize>(), 21);
    }

    /// 为什么要测（#256 A3）：副标题错了，只用 Codex 的新用户第一眼就觉得这个软件不是给自己的
    #[test]
    fn tagline_names_both_tools() {
        assert!(TAGLINE.contains("Claude Code") && TAGLINE.contains("Codex"), "{TAGLINE}");
    }

    /// 为什么要测（#256 A5）：信任说明里一句不真实的话比没有更糟。这里守住三条：说明了读哪、写哪、不上传；
    /// 没有「只读」（会建软链、点了才装 hook 会写 settings.json）；没有「联网」字样（用户明确不要写）
    #[test]
    fn trust_note_is_accurate_and_complete() {
        for must in ["~/.claude", "~/.codex", "~/.claude/makit", "不会上传"] {
            assert!(TRUST_NOTE.contains(must), "缺「{must}」：{TRUST_NOTE}");
        }
        for never in ["只读", "联网"] {
            assert!(!TRUST_NOTE.contains(never), "不能出现「{never}」：{TRUST_NOTE}");
        }
    }

    // ---- A4：新用户「3 步开始」/ 完整快捷键表折叠（#256）----

    /// 为什么要测：没有会话时如果还是直接显示完整表，新用户会被 20 多条快捷键淹没；
    /// 已经有会话时如果被当成新用户折叠掉，熟手每次关标签都要多点一下才能看到快捷键
    #[test]
    fn full_shortcuts_only_show_when_no_hint_or_user_expanded() {
        assert!(!show_full_shortcuts(true, false), "新用户、没点展开：折叠");
        assert!(show_full_shortcuts(true, true), "新用户点了「查看全部快捷键」：展开");
        assert!(show_full_shortcuts(false, false), "已经有会话：不折叠，不管 expanded");
        assert!(show_full_shortcuts(false, true), "已经有会话 + 碰巧 expanded=true：仍然显示");
    }

    #[test]
    fn three_steps_mention_the_two_tools_and_the_sidebar() {
        let all = STEPS.join("");
        assert_eq!(STEPS.len(), 3, "就是「3 步」，多一步少一步都要改标题措辞");
        assert!(all.contains("claude") && all.contains("codex"), "{all}");
        assert!(all.contains("侧栏") || all.contains("左侧"), "要呼应「会话自动出现在左侧」：{all}");
    }
}
