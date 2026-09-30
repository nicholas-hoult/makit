//! 「按项目」视图里一个项目组折没折（照搬 `projectCollapse.ts`）。
//!
//! 编码（一个集合存两侧，持久化在 `prefs.sidebar.proj_collapsed`，即 `makit-proj-collapsed`）：
//!   colKey                  = 用户显式折叠过
//!   "__expanded__" + colKey = 用户显式展开过
//! 之所以要两侧：默认值不是常量 —— 组里有活跃会话时默认展开，否则默认折叠。
//!
//! 为什么单独测：⌘L 定位要**反过来强制展开**，右键「只看这个项目」要写显式态，点表头要两侧都动；
//! 改一侧忘了删另一侧，界面上就是「点了没反应」「⌘L 定位不到」。用例移植自 `scripts/test-project-collapse.ts`。
//!
//! 集合用 `Vec<String>` 表示并保持插入顺序（和 JS 的 Set 一样），直接就是存盘的形状。

const EXPANDED_PREFIX: &str = "__expanded__";

fn expanded(key: &str) -> String {
    format!("{EXPANDED_PREFIX}{key}")
}

fn has(set: &[String], k: &str) -> bool {
    set.iter().any(|x| x == k)
}

fn remove(set: &mut Vec<String>, k: &str) {
    set.retain(|x| x != k);
}

fn add(set: &mut Vec<String>, k: String) {
    if !has(set, &k) {
        set.push(k);
    }
}

/// 一个项目组在折叠集合里的键：有仓库根用仓库根，没有退到组名
pub fn project_collapse_key(git_root: &str, name: &str) -> String {
    if git_root.is_empty() { name.to_string() } else { git_root.to_string() }
}

/// `has_active` = 组里有活着的会话，是**默认值**的来源
pub fn is_project_collapsed(set: &[String], key: &str, has_active: bool) -> bool {
    if has(set, key) {
        return true;
    }
    !has(set, &expanded(key)) && !has_active
}

/// 点表头：在两个显式态之间翻，两侧都要动
pub fn toggle_project_collapsed(set: &[String], key: &str, has_active: bool) -> Vec<String> {
    let mut next = set.to_vec();
    if is_project_collapsed(set, key, has_active) {
        remove(&mut next, key);
        add(&mut next, expanded(key));
    } else {
        remove(&mut next, &expanded(key));
        add(&mut next, key.to_string());
    }
    next
}

/// 右键「只看这个项目」：其余全部显式折叠，自己显式展开（这里**该**写显式态）
pub fn collapse_other_projects(set: &[String], all_keys: &[String], keep: &str) -> Vec<String> {
    let mut next = set.to_vec();
    for k in all_keys.iter().filter(|k| *k != keep) {
        remove(&mut next, &expanded(k));
        add(&mut next, k.clone());
    }
    remove(&mut next, keep);
    add(&mut next, expanded(keep));
    next
}

/// ⌘L 定位：把目标组强制展开。本来就展开着就返回 None（不偷偷把「默认」改成「显式展开」）
pub fn expand_project_for_reveal(set: &[String], key: &str, has_active: bool) -> Option<Vec<String>> {
    if !is_project_collapsed(set, key, has_active) {
        return None;
    }
    let mut next = set.to_vec();
    remove(&mut next, key);
    add(&mut next, expanded(key));
    Some(next)
}

/// 一键展开 / 折叠全部（#238）：只要有一个组是折叠的 → 全部展开；全都展开着 → 全部折叠。
/// `projects` 是 (折叠键, 组里有没有活跃会话)。写的都是**显式态**，不受「有活跃默认展开」影响
pub fn toggle_all_projects(set: &[String], projects: &[(String, bool)]) -> Vec<String> {
    let any_collapsed = projects.iter().any(|(k, active)| is_project_collapsed(set, k, *active));
    let mut next = set.to_vec();
    for (k, _) in projects {
        remove(&mut next, k);
        remove(&mut next, &expanded(k));
        add(&mut next, if any_collapsed { expanded(k) } else { k.clone() });
    }
    next
}

/// 状态视图 / 日期段：折叠集合里放的是组 id。同样是「有折叠的 → 全展开，否则全折叠」
pub fn toggle_all_groups(set: &[String], ids: &[String]) -> Vec<String> {
    let any_collapsed = ids.iter().any(|i| has(set, i));
    let mut next = set.to_vec();
    for i in ids {
        remove(&mut next, i);
        if !any_collapsed {
            add(&mut next, i.clone());
        }
    }
    next
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "/Users/x/makit";

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn key_uses_git_root_then_name() {
        assert_eq!(project_collapse_key(KEY, "makit"), KEY);
        assert_eq!(project_collapse_key("", "其他"), "其他");
    }

    #[test]
    fn default_depends_on_activity_and_explicit_states_win() {
        assert!(!is_project_collapsed(&[], KEY, true), "有活跃 session 的组默认展开");
        assert!(is_project_collapsed(&[], KEY, false), "全是已停止 session 的组默认折叠");
        assert!(is_project_collapsed(&s(&[KEY]), KEY, true), "显式折叠压过「有活跃」的默认展开");
        assert!(!is_project_collapsed(&[expanded(KEY)], KEY, false), "显式展开压过默认折叠");
        assert!(is_project_collapsed(&[KEY.to_string(), expanded(KEY)], KEY, false), "两个键同时在时以折叠为准");
    }

    #[test]
    fn toggle_flips_both_sides() {
        let orig = s(&[KEY]);
        let after_expand = toggle_project_collapsed(&orig, KEY, true);
        assert!(has(&after_expand, &expanded(KEY)), "从折叠点开：加上展开键");
        assert!(!has(&after_expand, KEY), "从折叠点开：删掉折叠键");
        assert!(!is_project_collapsed(&after_expand, KEY, false));
        let after_collapse = toggle_project_collapsed(&after_expand, KEY, false);
        assert!(has(&after_collapse, KEY), "再点一次：加上折叠键");
        assert!(!has(&after_collapse, &expanded(KEY)), "再点一次：删掉展开键");
        assert!(is_project_collapsed(&after_collapse, KEY, true));
        assert_eq!(orig, s(&[KEY]), "toggle 不改原集合");
    }

    #[test]
    fn reveal_forces_expand_but_leaves_default_open_alone() {
        let r = expand_project_for_reveal(&[], KEY, false).expect("默认折叠的要展开");
        assert!(!is_project_collapsed(&r, KEY, false), "定位到「默认折叠」的组 → 展开");
        let r = expand_project_for_reveal(&s(&[KEY]), KEY, false).expect("显式折叠的要展开");
        assert!(!is_project_collapsed(&r, KEY, false), "定位到「用户手动折叠过」的组 → 也展开");
        assert!(!has(&r, KEY), "删掉了折叠键，否则展开键会被压住");
        assert_eq!(expand_project_for_reveal(&[], KEY, true), None, "本来就展开的组不动（不触发重画、不写显式展开）");
        assert_eq!(expand_project_for_reveal(&[expanded(KEY)], KEY, false), None, "已经显式展开的也原样");
        let other = s(&[KEY, "/Users/x/other"]);
        assert!(has(&expand_project_for_reveal(&other, KEY, false).unwrap(), "/Users/x/other"), "不影响别的组");
    }

    #[test]
    fn only_this_project() {
        let (a, b, c) = ("/Users/x/a", "/Users/x/b", "/Users/x/c");
        let keys = s(&[a, b, c]);
        let only = collapse_other_projects(&[], &keys, b);
        assert!(has(&only, a) && has(&only, c), "其余被折叠");
        assert!(!has(&only, b), "B 自己不在折叠键里");
        assert!(has(&only, &expanded(b)), "B 写上了显式展开键");
        assert!(!has(&only, &expanded(a)));
        let had = vec![expanded(a), expanded(c)];
        let only2 = collapse_other_projects(&had, &keys, b);
        assert!(has(&only2, a) && !has(&only2, &expanded(a)), "原来显式展开的组会被真的折叠");
        assert_eq!(had, vec![expanded(a), expanded(c)], "入参不被修改");
        let keep_was_collapsed = collapse_other_projects(&s(&[b]), &s(&[a, b]), b);
        assert!(!is_project_collapsed(&keep_was_collapsed, b, false), "保留的组原来是折叠的，也要展开");
        assert!(is_project_collapsed(&only, a, true), "被折叠的组即使有活跃进程也压得住");
    }

    #[test]
    fn toggle_all_projects_expands_when_any_is_collapsed() {
        let projects = vec![("/a".to_string(), true), ("/b".to_string(), false), ("/c".to_string(), false)];
        // /a 有活跃默认展开；/b /c 默认折叠 → 有折叠的 → 全展开
        let next = toggle_all_projects(&[], &projects);
        for (k, active) in &projects {
            assert!(!is_project_collapsed(&next, k, *active), "{k} 该展开");
            assert!(!is_project_collapsed(&next, k, false), "{k} 是显式展开，没有活跃会话也不会被默认值压回去");
        }
    }

    #[test]
    fn toggle_all_projects_collapses_when_all_are_open() {
        let projects = vec![("/a".to_string(), true), ("/b".to_string(), true)];
        let next = toggle_all_projects(&[], &projects);
        for (k, _) in &projects {
            assert!(is_project_collapsed(&next, k, true), "{k} 该折叠，有活跃会话也一样");
        }
        // 再来一次又回到全展开
        let back = toggle_all_projects(&next, &projects);
        assert!(projects.iter().all(|(k, a)| !is_project_collapsed(&back, k, *a)));
    }

    #[test]
    fn toggle_all_projects_leaves_unlisted_keys_and_does_not_grow_forever() {
        let projects = vec![("/a".to_string(), false)];
        let start = s(&["/gone"]);
        let next = toggle_all_projects(&start, &projects);
        assert!(has(&next, "/gone"), "不在当前列表里的键原样保留");
        let mut cur = next;
        for _ in 0..6 {
            cur = toggle_all_projects(&cur, &projects);
        }
        assert!(cur.len() <= 2, "反复切不该越堆越多：{cur:?}");
    }

    #[test]
    fn toggle_all_groups_flips_between_none_and_all_collapsed() {
        let ids = s(&["attention", "busy", "idle", "2026-09-29"]);
        let collapsed_all = toggle_all_groups(&[], &ids);
        assert!(ids.iter().all(|i| has(&collapsed_all, i)), "全展开着 → 全折叠");
        let one = s(&["busy"]);
        let expanded_all = toggle_all_groups(&one, &ids);
        assert!(!ids.iter().any(|i| has(&expanded_all, i)), "有折叠的 → 全展开");
        assert!(has(&toggle_all_groups(&s(&["other"]), &ids), "other"), "别的键保留");
    }
}
