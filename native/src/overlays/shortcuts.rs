//! 设置页的快捷键参考表：**读 `actions::keymap()` 这张唯一的表**（修 #224「两份快捷键表」），
//! 这里只负责展示：键位写法转成 ⌘⇧ 符号、按轴分组、同一个 action 的几个键并成一行、
//! ⌘1…⌘9 这种连号并成「⌘1–⌘9」。
//!
//! 为什么单独测：改键位时只改 keymap，设置页自动跟上 —— 前提是这里的分组规则把每一条都放进了
//! 某个组（漏了就是「设置里查不到这个键」），测试逐条核对覆盖。

use crate::actions::{Owner, Shortcut};

/// GPUI 键位写法 → 显示（`alt-cmd-left` → `⌥⌘←`，修饰键按 Tauri 版设置页的写法 ⌃⌥⌘⇧ 排：⌘⇧D、⌥⌘←、⌃⇧Tab）
pub fn format_keys(keys: &str) -> String {
    let (mut ctrl, mut alt, mut shift, mut cmd) = (false, false, false, false);
    let mut rest = keys;
    loop {
        let before = rest;
        for (m, flag) in [("ctrl-", &mut ctrl), ("alt-", &mut alt), ("shift-", &mut shift), ("cmd-", &mut cmd)] {
            if let Some(r) = rest.strip_prefix(m) {
                // 「cmd--」：剥掉 cmd- 之后剩下的 "-" 就是键本身
                if !r.is_empty() {
                    *flag = true;
                    rest = r;
                }
            }
        }
        if rest == before {
            break;
        }
    }
    let key = match rest {
        "left" => "←".to_string(),
        "right" => "→".into(),
        "up" => "↑".into(),
        "down" => "↓".into(),
        "enter" => "↩".into(),
        "escape" => "Esc".into(),
        "tab" => "Tab".into(),
        "pageup" => "PgUp".into(),
        "pagedown" => "PgDn".into(),
        "backspace" => "⌫".into(),
        "delete" => "⌦".into(),
        "space" => "Space".into(),
        "home" => "Home".into(),
        "end" => "End".into(),
        k => k.to_uppercase(),
    };
    let mut out = String::new();
    for (on, sym) in [(ctrl, "⌃"), (alt, "⌥"), (cmd, "⌘"), (shift, "⇧")] {
        if on {
            out.push_str(sym);
        }
    }
    out + &key
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShortcutRow {
    /// 显示用的键（已格式化）。`range` 为真时是 [首, 尾]
    pub keys: Vec<String>,
    pub desc: String,
    pub range: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShortcutGroup {
    pub title: &'static str,
    pub rows: Vec<ShortcutRow>,
}

/// 分组顺序（照 Tauri 版设置页的五组，加一组「终端」）
pub const GROUP_TITLES: [&str; 6] = ["搜索 / 命令", "Tab（⌘ 轴）", "Pane（⌥⌘ 轴）", "侧栏 / 视图", "终端", "面板内"];

/// 输入框的编辑键不是 makit 的快捷键，不展示（浮层的单行输入框、侧栏搜索框）
pub fn is_listed(s: &Shortcut) -> bool {
    !matches!(s.context, Some("TextInput") | Some("SidebarSearch"))
}

/// 一条快捷键归哪一组（GROUP_TITLES 的下标）
pub fn group_of(s: &Shortcut) -> usize {
    match s.context {
        Some("Terminal") => return 4,
        Some(_) => return 5,
        None => {}
    }
    let name = s.action.name();
    if name.starts_with("workspace::") {
        return if name.contains("Split") || name.contains("FocusPane") || name.contains("Maximize") { 2 } else { 1 };
    }
    // ⌘⇧F 聚焦侧栏搜索：Tauri 版把它和 ⌘K / ⌘F 放在「搜索」一组
    if name.contains("Search") || name.contains("Find") {
        return 0;
    }
    if s.owner == Owner::Sidebar || name.ends_with("::ToggleSidebar") {
        return 3;
    }
    0
}

/// 整张表 → 分组展示
pub fn shortcut_groups(km: &[Shortcut]) -> Vec<ShortcutGroup> {
    // 每组：(行, 这一行对应 km 里哪一条的 action)
    let mut groups: Vec<Vec<(ShortcutRow, usize)>> = vec![Vec::new(); GROUP_TITLES.len()];
    for (i, s) in km.iter().enumerate().filter(|(_, s)| is_listed(s)) {
        let rows = &mut groups[group_of(s)];
        let key = format_keys(s.keys);
        match rows.iter_mut().find(|(_, j)| km[*j].action.partial_eq(s.action.as_ref())) {
            Some((row, _)) => {
                if !row.keys.contains(&key) {
                    row.keys.push(key);
                }
            }
            None => rows.push((ShortcutRow { keys: vec![key], desc: s.desc.to_string(), range: false }, i)),
        }
    }
    GROUP_TITLES
        .iter()
        .zip(groups)
        .map(|(title, rows)| ShortcutGroup { title, rows: collapse_numbered(rows.into_iter().map(|(r, _)| r).collect()) })
        .collect()
}

/// 「⌘1 第 1 个标签」…「⌘9 第 9 个标签」这种连号并成一行「⌘1–⌘9 第 N 个标签」
fn collapse_numbered(rows: Vec<ShortcutRow>) -> Vec<ShortcutRow> {
    // 单键、以数字 d 结尾、说明里含 d 的行 → (去掉数字的键前缀, 数字换成 N 的说明, d)
    fn numbered(r: &ShortcutRow) -> Option<(String, String, u32)> {
        let [k] = r.keys.as_slice() else { return None };
        let d = k.chars().last()?.to_digit(10)?;
        let ds = d.to_string();
        r.desc.contains(&ds).then(|| (k[..k.len() - 1].to_string(), r.desc.replacen(&ds, "N", 1), d))
    }
    let mut out: Vec<ShortcutRow> = Vec::new();
    let mut i = 0;
    while i < rows.len() {
        if let Some((prefix, desc, 1)) = numbered(&rows[i]) {
            let mut j = i + 1;
            while j < rows.len() && numbered(&rows[j]) == Some((prefix.clone(), desc.clone(), (j - i + 1) as u32)) {
                j += 1;
            }
            if j - i >= 3 {
                out.push(ShortcutRow { keys: vec![rows[i].keys[0].clone(), rows[j - 1].keys[0].clone()], desc, range: true });
                i = j;
                continue;
            }
        }
        out.push(rows[i].clone());
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::keymap;

    #[test]
    fn formats_mac_symbols() {
        assert_eq!(format_keys("cmd-shift-d"), "⌘⇧D");
        assert_eq!(format_keys("alt-cmd-left"), "⌥⌘←");
        assert_eq!(format_keys("alt-cmd-enter"), "⌥⌘↩");
        assert_eq!(format_keys("ctrl-shift-tab"), "⌃⇧Tab");
        assert_eq!(format_keys("cmd-,"), "⌘,");
        assert_eq!(format_keys("cmd-="), "⌘=");
        assert_eq!(format_keys("cmd--"), "⌘-", "减号本身就是分隔符");
        assert_eq!(format_keys("cmd-shift-["), "⌘⇧[");
        assert_eq!(format_keys("shift-pageup"), "⇧PgUp");
        assert_eq!(format_keys("escape"), "Esc");
        assert_eq!(format_keys("up"), "↑");
        assert_eq!(format_keys("shift-cmd-k"), "⌘⇧K", "修饰键顺序和写法无关");
    }

    #[test]
    fn every_listed_binding_lands_in_exactly_one_row() {
        let km = keymap();
        let groups = shortcut_groups(&km);
        assert_eq!(groups.iter().map(|g| g.title).collect::<Vec<_>>(), GROUP_TITLES.to_vec());
        for s in km.iter().filter(|s| is_listed(s)) {
            let shown = format_keys(s.keys);
            // 同一个键在不同面板里含义不同（↓ = 侧栏下一条 / 通知下一条 / ⌘K 下一项），各占一行；
            // 要核对的是：这一条落在**它自己 action 那一行**里，正好一次。行按 action 合并，说明取该 action 第一条的
            let desc = km.iter().find(|k| is_listed(k) && k.action.partial_eq(s.action.as_ref())).unwrap().desc;
            let hits: usize = groups
                .iter()
                .flat_map(|g| &g.rows)
                .filter(|r| r.desc == desc || r.range)
                .filter(|r| {
                    if r.range {
                        // 连号行：首尾之间的都算
                        let (a, b) = (&r.keys[0], &r.keys[1]);
                        let (pa, pb) = (&a[..a.len() - 1], &b[..b.len() - 1]);
                        pa == pb && shown.starts_with(pa) && shown.len() == a.len() && shown.as_str() >= a.as_str() && shown.as_str() <= b.as_str()
                    } else {
                        r.keys.contains(&shown)
                    }
                })
                .count();
            assert_eq!(hits, 1, "{} 应该正好出现在一行里", s.keys);
        }
        assert!(!groups.iter().flat_map(|g| &g.rows).any(|r| r.keys.iter().any(|k| k == "⌘A")), "输入框的编辑键不展示");
    }

    #[test]
    fn same_action_merges_and_numbers_collapse() {
        let groups = shortcut_groups(&keymap());
        let rows = |t: &str| groups.iter().find(|g| g.title == t).unwrap().rows.clone();
        let tab = rows("Tab（⌘ 轴）");
        let next = tab.iter().find(|r| r.desc == "下一个标签").unwrap();
        assert!(next.keys.contains(&"⌘]".to_string()) && next.keys.contains(&"⌃Tab".to_string()), "同一个 action 并成一行");
        let n = tab.iter().find(|r| r.range).unwrap();
        assert_eq!(n.keys, ["⌘1", "⌘9"]);
        assert_eq!(n.desc, "第 N 个标签");
        let pane = rows("Pane（⌥⌘ 轴）");
        assert!(pane.iter().any(|r| r.range && r.keys == ["⌥⌘1", "⌥⌘9"]));
        assert!(pane.iter().any(|r| r.keys == ["⌘D"]), "左右分屏归 pane 轴");
        assert!(pane.iter().any(|r| r.keys == ["⌥⌘↩"]));
        let search = rows("搜索 / 命令");
        for k in ["⌘K", "⌘F", "⌘⇧F", "⌘I", "⌘R", "⌘,"] {
            assert!(search.iter().any(|r| r.keys.contains(&k.to_string())), "{k} 在搜索 / 命令组");
        }
        let side = rows("侧栏 / 视图");
        assert!(side.iter().any(|r| r.keys == ["⌘B"]) && side.iter().any(|r| r.keys == ["⌘L"]));
        assert!(rows("面板内").iter().any(|r| r.keys == ["Esc"]));
        assert!(rows("终端").iter().any(|r| r.keys.contains(&"⌘=".to_string())));
        let _ = Owner::F0;
    }
}
