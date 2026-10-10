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
    /// Order in which records were inserted (monotonic); tells records seen before the last visibility pass from new ones
    pub seq: u64,
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
    // ---- incremental visibility (#269): what the last `recompute_visible` established ----
    /// Uuids on the active chain as of the last pass
    active: HashSet<String>,
    /// Reply ids (`message.id`) of the active chain
    active_msgs: HashSet<String>,
    /// Reply ids of the records whose entries are already classified
    seen_msgs: HashSet<String>,
    /// `all[..classified]` has been judged visible or not
    classified: usize,
    /// The leaf the active chain was built for
    chain_leaf: Option<String>,
    /// `rec_seq` at the end of the last pass: records with a smaller `seq` are "old"
    classified_seq: u64,
    rec_seq: u64,
    /// A uuid was inserted twice: the earlier record's data changed under entries already classified
    rec_replaced: bool,
    /// Uuids inserted since the last pass
    new_uuids: Vec<String>,
    /// Parent uuids that were missing when a non-conversation entry was judged (it counted as visible);
    /// if such a record shows up later the entry may flip
    dangling: HashSet<String>,
    full_recomputes: usize,
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
            active: HashSet::new(),
            active_msgs: HashSet::new(),
            seen_msgs: HashSet::new(),
            classified: 0,
            chain_leaf: None,
            classified_seq: 0,
            rec_seq: 0,
            rec_replaced: false,
            new_uuids: Vec::new(),
            dangling: HashSet::new(),
            full_recomputes: 0,
        }
    }

    /// Register a record. Records get a sequence number so that the incremental pass can tell new records from old ones
    pub(super) fn insert_rec(&mut self, uuid: &str, mut rec: Rec) {
        self.rec_seq += 1;
        rec.seq = self.rec_seq;
        if self.recs.insert(uuid.to_string(), rec).is_some() {
            self.rec_replaced = true;
        }
        self.new_uuids.push(uuid.to_string());
    }

    /// How many times the whole visible set was rebuilt (tests: appending must not do this)
    pub fn full_recomputes(&self) -> usize {
        self.full_recomputes
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

    /// Bring the visible set up to date. Returns true when the list before the call is still a prefix of the list after it
    /// (only appends happened), false when something earlier changed and the consumer has to rebuild its view.
    ///
    /// The common case (a live session growing) is handled in O(new records): the active chain only grows at its
    /// end, and no earlier entry can change visibility. Anything else (rewind, compaction, a reply whose earlier
    /// sibling is off the chain, a record seen twice) falls back to the full pass.
    pub fn recompute_visible(&mut self) -> bool {
        if self.try_extend_visible() {
            return true;
        }
        self.full_recompute()
    }

    /// Fast path. Returns false (state untouched) when the full pass is needed
    fn try_extend_visible(&mut self) -> bool {
        if self.rec_replaced || self.new_uuids.iter().any(|u| self.dangling.contains(u)) {
            return false;
        }
        if self.tool == Tool::Codex {
            self.visible.extend(self.classified..self.all.len());
            self.classified = self.all.len();
            return true;
        }
        let mut grown: Vec<String> = Vec::new();
        if self.leaf != self.chain_leaf {
            // Only the shape "new leaf hangs off the old leaf through records added since" is an extension
            let Some(old_leaf) = self.chain_leaf.as_deref() else { return false };
            let mut cur = self.leaf.as_deref();
            loop {
                let Some(uuid) = cur else { return false };
                if uuid == old_leaf {
                    break;
                }
                // Reaching any other known chain member means the leaf moved back (rewind); a loop means a cycle
                if self.active.contains(uuid) || grown.len() > self.recs.len() {
                    return false;
                }
                let Some(r) = self.recs.get(uuid) else { return false };
                // A conversation record that was already judged would flip from hidden to visible in the middle of the list
                if r.conv && r.seq < self.classified_seq {
                    return false;
                }
                // A reply that starts being active while an earlier block of it was already judged (hidden) would reveal that block
                if let Some(m) = r.msg.as_deref() {
                    if !self.active_msgs.contains(m) && self.seen_msgs.contains(m) {
                        return false;
                    }
                }
                grown.push(uuid.to_string());
                cur = r.parent.as_deref().or(r.logical_parent.as_deref());
            }
        }
        for uuid in grown {
            if let Some(m) = self.recs.get(&uuid).and_then(|r| r.msg.clone()) {
                self.active_msgs.insert(m);
            }
            self.active.insert(uuid);
        }
        self.chain_leaf = self.leaf.clone();
        self.classify_new_entries();
        true
    }

    /// Judge `all[classified..]` against the current active chain and append the visible ones
    fn classify_new_entries(&mut self) {
        let mut dangling: Vec<String> = Vec::new();
        for i in self.classified..self.all.len() {
            let rec = self.all[i].rec.as_deref().and_then(|u| self.recs.get(u).map(|r| (u, r)));
            // Codex has no chain; an entry without a (known) record is shown
            let show = self.tool == Tool::Codex
                || match rec {
                    None => true,
                    Some((uuid, r)) if r.conv => self.active.contains(uuid) || r.msg.as_deref().is_some_and(|m| self.active_msgs.contains(m)),
                    Some((uuid, _)) => match self.anchor_in(uuid) {
                        Ok(shown) => shown,
                        Err(missing) => {
                            dangling.push(missing);
                            true
                        }
                    },
                };
            if let Some(m) = rec.and_then(|(_, r)| r.msg.as_deref()) {
                if !self.seen_msgs.contains(m) {
                    self.seen_msgs.insert(m.to_string());
                }
            }
            if show {
                self.visible.push(i);
            }
        }
        self.dangling.extend(dangling);
        self.classified = self.all.len();
        self.classified_seq = self.rec_seq;
        self.new_uuids.clear();
    }

    /// Recompute the visible set from the current branch, from scratch
    fn full_recompute(&mut self) -> bool {
        self.full_recomputes += 1;
        let old = std::mem::take(&mut self.visible);
        let chain = self.active_chain();
        let active: HashSet<String> = chain.iter().map(|u| u.to_string()).collect();
        // Replies seen on the active chain: the other blocks of the same reply are visible too
        let active_msgs: HashSet<String> = chain.iter().filter_map(|u| self.recs.get(*u)?.msg.clone()).collect();
        drop(chain);
        self.active = active;
        self.active_msgs = active_msgs;
        self.seen_msgs.clear();
        self.classified = 0;
        self.rec_replaced = false;
        self.dangling.clear();
        self.chain_leaf = self.leaf.clone();
        self.classify_new_entries();
        old.len() <= self.visible.len() && self.visible[..old.len()] == old[..]
    }

    /// Non-conversation records (system etc.): walk up to the nearest conversation-record ancestor; visible if it is on the current branch; also visible if there is no conversation ancestor all the way up (e.g. a leading system record).
    /// Looking only at the direct parent is not enough: a series of system records can hang off each other (turn_duration -> another system), and they all count as hanging under the same conversation record.
    /// `Err(uuid)`: the walk ended at a record that is not (yet) known; the entry counts as visible but may flip when it appears
    fn anchor_in<'a>(&'a self, uuid: &'a str) -> Result<bool, String> {
        let mut cur = uuid;
        for _ in 0..64 {
            let Some(r) = self.recs.get(cur) else { return Err(cur.to_string()) };
            if r.conv {
                return Ok(self.active.contains(cur));
            }
            match r.parent.as_deref().or(r.logical_parent.as_deref()) {
                None => return Ok(true),
                Some(p) => cur = p,
            }
        }
        Ok(true)
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
