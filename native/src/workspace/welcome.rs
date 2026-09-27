//! 欢迎卡（pane 里一个标签都没有时显示，`ContainerView.tsx` 的 `.welcome-card`）。
//!
//! 键位**读单一快捷键表**（`actions::keymap()`）：每一行写的是 GPUI 键位串，显示文字由 `key_label` 从键位串
//! 推出来，测试检查每个键都真的在表里 —— 改了快捷键忘了改欢迎卡，测试会红（修 #224「两份快捷键表」）。
//! 说明文字沿用 Tauri 版欢迎卡的措辞（比表里的 desc 更口语，且按「轴」分组在教规则），不从表里取。
//! 分组、顺序、键帽的拆法（修饰键和方向键分两个键帽）照搬 TS 版。

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
}
