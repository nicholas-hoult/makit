//! Sidebar multi-selection (#294): ⌘-click toggles a row, ⇧-click selects a range from the anchor (in visible row
//! order), and the batch archive decides which of the selected sessions it may archive. Pure data, no GPUI.

/// The multi-selection: ids in the order they were added, plus the anchor ⇧-click ranges start from
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MultiSelect {
    ids: Vec<String>,
    anchor: Option<String>,
}

impl MultiSelect {
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn contains(&self, id: &str) -> bool {
        self.ids.iter().any(|x| x == id)
    }

    pub fn ids(&self) -> &[String] {
        &self.ids
    }

    pub fn clear(&mut self) {
        self.ids.clear();
        self.anchor = None;
    }

    /// Drop ids that are no longer in the list (archived away, filtered out by search). Keeps the anchor only if it
    /// survives. A cancelled batch confirm leaves the selection as it was; a done batch prunes itself on the rebuild.
    pub fn retain(&mut self, keep: impl Fn(&str) -> bool) {
        self.ids.retain(|id| keep(id));
        if self.anchor.as_deref().is_some_and(|a| !keep(a)) {
            self.anchor = None;
        }
    }

    /// ⌘-click. `current` is the plainly selected row: starting a multi-selection keeps it (as Finder does), so
    /// click A then ⌘-click B selects both. The clicked row becomes the anchor.
    pub fn toggle(&mut self, id: &str, current: Option<&str>) {
        if self.ids.is_empty() {
            if let Some(c) = current {
                self.ids.push(c.to_string());
            }
        }
        if let Some(i) = self.ids.iter().position(|x| x == id) {
            self.ids.remove(i);
        } else {
            self.ids.push(id.to_string());
        }
        self.anchor = Some(id.to_string());
    }

    /// ⇧-click: replace the selection with the rows between the anchor (else `current`, else `id` itself) and `id`,
    /// inclusive, in `order` (the visible rows top to bottom). The anchor stays, so successive ⇧-clicks pivot on it.
    pub fn select_range(&mut self, order: &[String], id: &str, current: Option<&str>) {
        let anchor = self.anchor.clone().or_else(|| current.map(str::to_string)).unwrap_or_else(|| id.to_string());
        let (from, to) = match (order.iter().position(|x| *x == anchor), order.iter().position(|x| x == id)) {
            (Some(a), Some(b)) => (a.min(b), a.max(b)),
            // anchor not visible any more (or the clicked row isn't in the order): start over at the clicked row
            _ => {
                self.ids = vec![id.to_string()];
                self.anchor = Some(id.to_string());
                return;
            }
        };
        self.ids.clear();
        for x in &order[from..=to] {
            if !self.ids.contains(x) {
                self.ids.push(x.clone());
            }
        }
        self.anchor = Some(anchor);
    }
}

/// What the batch archive needs to know about one selected session
#[derive(Clone, Debug, Default)]
pub struct Candidate {
    pub id: String,
    pub archived: bool,
    pub pinned: bool,
    pub running: bool,
    /// in a tab of this window
    pub opened: bool,
}

/// What one "Archive selected" does
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ArchivePlan {
    /// to archive (not archived yet, not pinned), in selection order
    pub ids: Vec<String>,
    /// of `ids`, how many are running or open in a tab: archiving closes their tabs and stops their processes
    pub active: usize,
    /// pinned ones are never archived by a batch; the toast reports how many were left alone
    pub pinned_skipped: usize,
}

impl ArchivePlan {
    /// Ask once before a batch that would close tabs or stop processes (the single archive asks only for running)
    pub fn needs_confirm(&self) -> bool {
        self.active > 0
    }
}

pub fn plan_archive(cands: &[Candidate]) -> ArchivePlan {
    let mut plan = ArchivePlan::default();
    for c in cands.iter().filter(|c| !c.archived) {
        if c.pinned {
            plan.pinned_skipped += 1;
            continue;
        }
        if c.running || c.opened {
            plan.active += 1;
        }
        plan.ids.push(c.id.clone());
    }
    plan
}

/// "Unarchive selected" is offered only when every selected session is archived; returns their ids then
pub fn unarchive_ids(cands: &[Candidate]) -> Option<Vec<String>> {
    (!cands.is_empty() && cands.iter().all(|c| c.archived)).then(|| cands.iter().map(|c| c.id.clone()).collect())
}

#[cfg(test)]
mod tests {
    //! Tested separately because the batch acts on many sessions at once: a wrong range or a wrong plan means the
    //! sidebar archives rows the user did not highlight, or silently archives a pinned / running session
    //! (closing its terminal) — mistakes the user only notices after the rows disappear.
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn cmd_click_keeps_the_plain_selection_and_toggles() {
        let mut m = MultiSelect::default();
        m.toggle("b", Some("a"));
        assert_eq!(m.ids(), s(&["a", "b"]), "the plainly selected row joins the multi-selection");
        m.toggle("c", Some("b"));
        assert_eq!(m.ids(), s(&["a", "b", "c"]));
        m.toggle("a", Some("c"));
        assert_eq!(m.ids(), s(&["b", "c"]), "⌘-click on a selected row removes it");
    }

    #[test]
    fn cmd_click_on_the_plain_selection_itself_deselects_it() {
        let mut m = MultiSelect::default();
        m.toggle("a", Some("a"));
        assert!(m.is_empty());
        m.toggle("a", None);
        assert_eq!(m.ids(), s(&["a"]));
    }

    #[test]
    fn shift_click_selects_the_range_from_the_anchor_both_ways() {
        let order = s(&["a", "b", "c", "d", "e"]);
        let mut m = MultiSelect::default();
        m.select_range(&order, "d", Some("b"));
        assert_eq!(m.ids(), s(&["b", "c", "d"]), "no anchor yet: start from the plain selection");
        m.select_range(&order, "a", Some("d"));
        assert_eq!(m.ids(), s(&["a", "b"]), "the anchor stays at b; the range is replaced, not extended");
        m.toggle("e", Some("a"));
        m.select_range(&order, "c", Some("e"));
        assert_eq!(m.ids(), s(&["c", "d", "e"]), "⌘-click moves the anchor");
    }

    #[test]
    fn shift_click_without_any_selection_selects_just_that_row() {
        let order = s(&["a", "b"]);
        let mut m = MultiSelect::default();
        m.select_range(&order, "b", None);
        assert_eq!(m.ids(), s(&["b"]));
    }

    #[test]
    fn shift_click_when_the_anchor_is_no_longer_visible_selects_just_that_row() {
        let mut m = MultiSelect::default();
        m.toggle("gone", None);
        m.select_range(&s(&["a", "b", "c"]), "c", None);
        assert_eq!(m.ids(), s(&["c"]));
    }

    #[test]
    fn range_never_holds_duplicates() {
        let mut m = MultiSelect::default();
        m.select_range(&s(&["a", "b", "a", "c"]), "c", Some("a"));
        assert_eq!(m.ids(), s(&["a", "b", "c"]));
    }

    #[test]
    fn clear_drops_ids_and_anchor() {
        let mut m = MultiSelect::default();
        m.toggle("a", None);
        m.clear();
        assert!(m.is_empty());
        m.select_range(&s(&["a", "b", "c"]), "c", None);
        assert_eq!(m.ids(), s(&["c"]), "the old anchor must not survive a clear");
    }

    #[test]
    fn retain_prunes_ids_and_a_vanished_anchor() {
        let mut m = MultiSelect::default();
        m.toggle("a", None);
        m.toggle("b", None);
        m.retain(|id| id != "b");
        assert_eq!(m.ids(), s(&["a"]));
        m.select_range(&s(&["a", "b", "c"]), "c", None);
        assert_eq!(m.ids(), s(&["c"]), "anchor b was pruned, so ⇧-click starts over at the clicked row");
    }

    fn c(id: &str, archived: bool, pinned: bool, running: bool, opened: bool) -> Candidate {
        Candidate { id: id.into(), archived, pinned, running, opened }
    }

    #[test]
    fn plan_skips_pinned_and_already_archived_and_counts_active() {
        let p = plan_archive(&[
            c("idle", false, false, false, false),
            c("pinned", false, true, true, false),
            c("running", false, false, true, false),
            c("opened", false, false, false, true),
            c("done", true, false, false, false),
        ]);
        assert_eq!(p.ids, s(&["idle", "running", "opened"]));
        assert_eq!(p.active, 2, "running and open-in-a-tab both close something");
        assert_eq!(p.pinned_skipped, 1);
        assert!(p.needs_confirm());
    }

    #[test]
    fn plan_of_quiet_sessions_needs_no_confirm() {
        let p = plan_archive(&[c("a", false, false, false, false), c("b", false, false, false, false)]);
        assert_eq!(p.ids, s(&["a", "b"]));
        assert!(!p.needs_confirm());
    }

    #[test]
    fn unarchive_is_offered_only_when_all_are_archived() {
        assert_eq!(unarchive_ids(&[c("a", true, false, false, false), c("b", true, true, false, false)]), Some(s(&["a", "b"])));
        assert_eq!(unarchive_ids(&[c("a", true, false, false, false), c("b", false, false, false, false)]), None);
        assert_eq!(unarchive_ids(&[]), None);
    }
}
