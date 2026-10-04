//! Parsing state machine: fed line by line, it accumulates all Items, then computes the visible ones from the "current branch" (rules in TRD section 11.4).
//!
//! Why "all Items" and "visible" are kept separate:
//! - A rewind leaves a dead branch in the file, so the current branch must be computed by following the `parentUuid` chain
//! - But a tool_result must attach back to its tool_use by id, regardless of whether it is on the chain itself (results of parallel tool calls are not on the final chain)
//! - After compaction `parentUuid` is broken, so `logicalParentUuid` reconnects to what came before the compaction

use std::collections::{BTreeMap, HashMap, HashSet};

use serde_json::Value;

use super::model::{Item, ItemKind, Tool};
use super::{claude, codex};

/// A record with a uuid (kept whether or not it produced an Item: it may be another record's parent)
pub(super) struct Rec {
    pub parent: Option<String>,
    pub logical_parent: Option<String>,
    /// A user / assistant conversation record (can be a leaf; others such as system are visible only when their parent is on the chain)
    pub conv: bool,
    /// The API reply this assistant record belongs to (`message.id`). One reply is split into several blocks (parallel tool calls...), and a later block's parent pointer
    /// may skip one of its siblings; these blocks are the same reply, not abandoned branches
    pub msg: Option<String>,
}

pub(super) struct Entry {
    pub item: Item,
    /// Which record this Item came from (key of `recs`: the uuid); None for codex, which has no chain
    pub rec: Option<String>,
}

pub struct State {
    pub(super) tool: Tool,
    pub(super) all: Vec<Entry>,
    pub(super) recs: HashMap<String, Rec>,
    /// call_id -> index into `all`
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

    /// Feed one line (without the newline). Returns which existing ToolCalls had their results updated by this line (indices into `all`)
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

    /// Recompute the visible set from the current branch
    pub fn recompute_visible(&mut self) {
        let active = self.active_chain();
        // Replies seen on the active chain: the other blocks of the same reply are visible too
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
                    // system etc.: walk up the parents to the nearest conversation record; visible only if it is on the chain
                    Some(_) => self.anchor_active(uuid, &active),
                },
            })
            .map(|(i, _)| i)
            .collect();
    }

    /// Non-conversation records (system etc.): walk up to the nearest conversation-record ancestor; visible if it is on the current branch; also visible if there is no conversation ancestor all the way up (e.g. a leading system record).
    /// Looking only at the direct parent is not enough: a series of system records can hang off each other (turn_duration -> another system), and they all count as hanging under the same conversation record
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

    /// Walk up from the last conversation record: `parentUuid`, or `logicalParentUuid` when it is empty (after compaction)
    fn active_chain(&self) -> HashSet<&str> {
        let mut set = HashSet::new();
        let mut cur = self.leaf.as_deref();
        while let Some(uuid) = cur {
            if !set.insert(uuid) {
                break; // cycle guard
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

    /// Index into `all` of the i-th item of the visible list
    pub fn visible_index(&self, i: usize) -> usize {
        self.visible[i]
    }

    /// Position in the visible list of an index into `all` (None if not visible)
    pub fn visible_pos(&self, all_ix: usize) -> Option<usize> {
        self.visible.binary_search(&all_ix).ok()
    }

    pub fn unknown_kinds(&self) -> &BTreeMap<String, usize> {
        &self.unknown
    }

    pub fn invalid_lines(&self) -> usize {
        self.invalid_lines
    }

    /// Attach a result to a ToolCall; returns whether it really attached
    pub(super) fn attach_result(&mut self, call_id: &str, result: super::model::ToolResult) -> Option<usize> {
        let ix = *self.calls.get(call_id)?;
        if let ItemKind::ToolCall { result: slot, .. } = &mut self.all[ix].item.kind {
            *slot = Some(result);
            return Some(ix);
        }
        None
    }
}
