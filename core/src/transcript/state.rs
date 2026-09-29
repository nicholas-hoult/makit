//! 解析状态机：逐行喂，攒出全部 Item，再按「当前分支」算出可见的那些（规则见 TRD §11.4）。
//!
//! 为什么「全部 Item」和「可见」分开：
//! - 回退（rewind）会在文件里留下废分支，要按 `parentUuid` 链算出当前分支
//! - 但 tool_result 要按 id 挂回 tool_use，不能看它自己在不在链上（并行工具调用的结果不在最终链上）
//! - 压缩之后 `parentUuid` 断了，靠 `logicalParentUuid` 接回压缩之前

use std::collections::{BTreeMap, HashMap, HashSet};

use serde_json::Value;

use super::model::{Item, ItemKind, Tool};
use super::{claude, codex};

/// 一条带 uuid 的记录（不管有没有产出 Item，都要留着：它可能是别的记录的父）
pub(super) struct Rec {
    pub parent: Option<String>,
    pub logical_parent: Option<String>,
    /// user / assistant 的对话记录（可以当叶子；其余的 system 等只在父在链上时才可见）
    pub conv: bool,
    /// assistant 记录所属的 API 回复（`message.id`）。一条回复会拆成多个块（并行工具调用……），后来的块的父指针
    /// 可以跳过其中一个兄弟；这些块是同一条回复，不是被放弃的分支
    pub msg: Option<String>,
}

pub(super) struct Entry {
    pub item: Item,
    /// 这个 Item 来自哪条记录（`recs` 的键：uuid）；codex 没有链，为 None
    pub rec: Option<String>,
}

pub struct State {
    pub(super) tool: Tool,
    pub(super) all: Vec<Entry>,
    pub(super) recs: HashMap<String, Rec>,
    /// call_id → `all` 里的下标
    pub(super) calls: HashMap<String, usize>,
    pub(super) leaf: Option<String>,
    pub(super) unknown: BTreeMap<String, usize>,
    pub(super) invalid_lines: usize,
    pub(super) codex: codex::Dedup,
    visible: Vec<usize>,
    lines: usize,
}

impl State {
    pub fn new(tool: Tool) -> Self {
        Self {
            tool,
            all: Vec::new(),
            recs: HashMap::new(),
            calls: HashMap::new(),
            leaf: None,
            unknown: BTreeMap::new(),
            invalid_lines: 0,
            codex: codex::Dedup::default(),
            visible: Vec::new(),
            lines: 0,
        }
    }

    /// 喂一行（不含换行）。返回这一行让哪些已有的 ToolCall 更新了结果（`all` 里的下标）
    pub fn feed_line(&mut self, line: &str) -> Vec<usize> {
        self.lines += 1;
        let line = line.trim();
        if line.is_empty() {
            return vec![];
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            self.invalid_lines += 1;
            return vec![];
        };
        if !v.is_object() {
            self.invalid_lines += 1;
            return vec![];
        }
        match self.tool {
            Tool::Claude => claude::feed(self, &v),
            Tool::Codex => codex::feed(self, &v, self.lines),
        }
    }

    pub(super) fn push(&mut self, rec: Option<&str>, item: Item) -> usize {
        self.all.push(Entry { item, rec: rec.map(String::from) });
        self.all.len() - 1
    }

    pub(super) fn count_unknown(&mut self, what: String) {
        *self.unknown.entry(what).or_default() += 1;
    }

    /// 按当前分支重算可见集合
    pub fn recompute_visible(&mut self) {
        let active = self.active_chain();
        // 活动链上出现过的回复：同一条回复的其余块也可见
        let active_msgs: HashSet<&str> = active.iter().filter_map(|u| self.recs.get(*u)?.msg.as_deref()).collect();
        self.visible = self
            .all
            .iter()
            .enumerate()
            .filter(|(_, e)| match (&self.tool, &e.rec) {
                (Tool::Codex, _) | (_, None) => true,
                (_, Some(uuid)) => match self.recs.get(uuid) {
                    None => true,
                    Some(r) if r.conv => active.contains(uuid.as_str()) || r.msg.as_deref().is_some_and(|m| active_msgs.contains(m)),
                    // system 等：顺着父往上找到最近的一条对话记录，它在链上才可见
                    Some(_) => self.anchor_active(uuid, &active),
                },
            })
            .map(|(i, _)| i)
            .collect();
    }

    /// 非对话记录（system 等）：往上找最近的对话记录祖先，它在当前分支上就可见；一路没有对话祖先（比如开头的 system）也可见。
    /// 不能只看直接的父：一串 system 记录可以互相挂着（turn_duration → 别的 system），它们都算挂在同一条对话记录下面
    fn anchor_active(&self, uuid: &str, active: &HashSet<&str>) -> bool {
        let mut cur = uuid;
        for _ in 0..64 {
            let Some(r) = self.recs.get(cur) else { return true };
            if r.conv {
                return active.contains(cur);
            }
            match r.parent.as_deref().or(r.logical_parent.as_deref()) {
                None => return true,
                Some(p) => cur = p,
            }
        }
        true
    }

    /// 从最后一条对话记录往上走：`parentUuid`，为空时改走 `logicalParentUuid`（压缩之后）
    fn active_chain(&self) -> HashSet<&str> {
        let mut set = HashSet::new();
        let mut cur = self.leaf.as_deref();
        while let Some(uuid) = cur {
            if !set.insert(uuid) {
                break; // 防环
            }
            cur = self.recs.get(uuid).and_then(|r| r.parent.as_deref().or(r.logical_parent.as_deref()));
        }
        set
    }

    pub fn visible_len(&self) -> usize {
        self.visible.len()
    }

    pub fn visible(&self, i: usize) -> &Item {
        &self.all[self.visible[i]].item
    }

    /// 可见列表里第 i 项在 `all` 里的下标
    pub fn visible_index(&self, i: usize) -> usize {
        self.visible[i]
    }

    /// `all` 里的下标在可见列表里的位置（不可见返回 None）
    pub fn visible_pos(&self, all_ix: usize) -> Option<usize> {
        self.visible.binary_search(&all_ix).ok()
    }

    pub fn unknown_kinds(&self) -> &BTreeMap<String, usize> {
        &self.unknown
    }

    pub fn invalid_lines(&self) -> usize {
        self.invalid_lines
    }

    /// 给 ToolCall 挂结果；返回是否真的挂上了
    pub(super) fn attach_result(&mut self, call_id: &str, result: super::model::ToolResult) -> Option<usize> {
        let ix = *self.calls.get(call_id)?;
        if let ItemKind::ToolCall { result: slot, .. } = &mut self.all[ix].item.kind {
            *slot = Some(result);
            return Some(ix);
        }
        None
    }
}
