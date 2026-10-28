//! opencode's conversation (#284): `message` + `part` rows -> the unified `Item` list.
//!
//! The data is not a file with an append-only tail, so the reader works differently from claude / codex: it re-reads the session when the
//! database says the session changed (see `OpencodeSource`) and reports what differs from the previous read as appended / updated.
//!
//! Part types measured on opencode 1.19.3 (`text`, `reasoning`, `tool`, `step-start`, `step-finish`) are mapped; the rest are ignored
//! on purpose (they carry no conversation: step markers, snapshots, patches).

use std::path::PathBuf;

use chrono::TimeZone;
use rusqlite::Connection;
use serde_json::Value;

use super::model::{DividerKind, Item, ItemKind, Level, ToolResult};
use super::reader::Changes;

/// One `message` + `part` pair as stored.
#[derive(Debug, Clone)]
pub struct Row {
    pub message_data: String,
    pub part_id: String,
    pub part_time_ms: i64,
    pub part_data: String,
}

fn iso(ms: i64) -> Option<String> {
    chrono::Utc.timestamp_millis_opt(ms).single().map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
}

/// Rows (ordered by message time, then part id) -> Items.
pub fn items_from_rows(rows: &[Row]) -> Vec<Item> {
    let mut out = Vec::new();
    let mut last_message: Option<&str> = None;
    for r in rows {
        let Ok(msg) = serde_json::from_str::<Value>(&r.message_data) else { continue };
        let Ok(part) = serde_json::from_str::<Value>(&r.part_data) else { continue };
        let role = msg.get("role").and_then(Value::as_str).unwrap_or("");
        let time = iso(r.part_time_ms);
        let mut push = |kind: ItemKind| out.push(Item { id: r.part_id.clone(), time: time.clone(), kind });
        // an assistant message that failed (auth, rate limit, ...) carries the reason on the message itself, once
        if role == "assistant" && last_message != Some(r.message_data.as_str()) {
            if let Some(text) = msg.pointer("/error/data/message").and_then(Value::as_str).or_else(|| msg.pointer("/error/name").and_then(Value::as_str)) {
                push(ItemKind::Notice { level: Level::Error, text: text.to_string() });
            }
        }
        last_message = Some(r.message_data.as_str());
        match part.get("type").and_then(Value::as_str).unwrap_or("") {
            "text" => {
                if part.get("synthetic").and_then(Value::as_bool) == Some(true) {
                    continue;
                }
                let text = part.get("text").and_then(Value::as_str).unwrap_or("");
                if text.trim().is_empty() {
                    continue;
                }
                match role {
                    "user" => push(ItemKind::User(text.to_string())),
                    "assistant" => push(ItemKind::Assistant(text.to_string())),
                    _ => {}
                }
            }
            "reasoning" => {
                let text = part.get("text").and_then(Value::as_str).unwrap_or("");
                if !text.trim().is_empty() {
                    push(ItemKind::Thinking(text.to_string()));
                }
            }
            "tool" => {
                let state = part.get("state").cloned().unwrap_or(Value::Null);
                let result = match state.get("status").and_then(Value::as_str).unwrap_or("") {
                    "completed" => Some(ToolResult::new(state.get("output").and_then(Value::as_str).unwrap_or(""), false, 0)),
                    "error" => Some(ToolResult::new(state.get("error").and_then(Value::as_str).unwrap_or(""), true, 0)),
                    // pending / running: the result arrives later, the row is re-read then
                    _ => None,
                };
                push(ItemKind::ToolCall {
                    call_id: part.get("callID").and_then(Value::as_str).unwrap_or("").to_string(),
                    name: part.get("tool").and_then(Value::as_str).unwrap_or("").to_string(),
                    input: state.get("input").cloned().unwrap_or(Value::Null),
                    result,
                });
            }
            "compaction" => push(ItemKind::Divider(DividerKind::Compacted)),
            _ => {}
        }
    }
    out
}

/// What changed in the session since a read that produced `old`, as the reader's `Changes`.
/// New items at the end are `appended`; an earlier item that differs (a tool call that got its result, a reply that grew) is `updated`;
/// the list getting shorter means the history was rewritten: `reset`.
pub fn diff(old: &[Item], new: &[Item]) -> Changes {
    if new.len() < old.len() {
        return Changes { appended: 0..new.len(), updated: vec![], reset: true };
    }
    let updated = (0..old.len()).filter(|&i| old[i] != new[i]).collect();
    Changes { appended: old.len()..new.len(), updated, reset: false }
}

/// Cheap "did the session change" probe: (max part update time, part count, max message update time).
pub type Fingerprint = (i64, i64, i64);

pub fn fingerprint(conn: &Connection, session_id: &str) -> rusqlite::Result<Fingerprint> {
    let (pt, pc): (i64, i64) = conn.query_row(
        "SELECT coalesce(max(time_updated), 0), count(*) FROM part WHERE session_id = ?1",
        [session_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let mt: i64 = conn.query_row("SELECT coalesce(max(time_updated), 0) FROM message WHERE session_id = ?1", [session_id], |r| r.get(0))?;
    Ok((pt, pc, mt))
}

pub fn read_rows(conn: &Connection, session_id: &str) -> rusqlite::Result<Vec<Row>> {
    let mut stmt = conn.prepare(
        "SELECT m.data, p.id, p.time_created, p.data FROM message m JOIN part p ON p.message_id = m.id \
         WHERE m.session_id = ?1 ORDER BY m.time_created, m.id, p.id",
    )?;
    let rows = stmt
        .query_map([session_id], |r| {
            Ok(Row { message_data: r.get(0)?, part_id: r.get(1)?, part_time_ms: r.get(2)?, part_data: r.get(3)? })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// The reader's state for an opencode session.
pub struct OpencodeSource {
    pub(super) db: PathBuf,
    pub(super) session_id: String,
    pub(super) items: Vec<Item>,
    pub(super) seen: Option<Fingerprint>,
}

impl OpencodeSource {
    pub fn new(db: PathBuf, session_id: String) -> Self {
        Self { db, session_id, items: vec![], seen: None }
    }

    /// Re-read if the database says the session changed; report the difference.
    pub fn poll(&mut self) -> std::io::Result<Changes> {
        let n = self.items.len();
        let io = |e: String| std::io::Error::new(std::io::ErrorKind::Other, e);
        let conn = crate::opencode::open_read_only(&self.db).map_err(io)?;
        let fp = fingerprint(&conn, &self.session_id).map_err(|e| io(e.to_string()))?;
        if self.seen == Some(fp) {
            return Ok(Changes { appended: n..n, ..Default::default() });
        }
        let rows = read_rows(&conn, &self.session_id).map_err(|e| io(e.to_string()))?;
        let new = items_from_rows(&rows);
        let changes = diff(&self.items, &new);
        self.items = new;
        self.seen = Some(fp);
        Ok(changes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn row(role: &str, part_id: &str, part: Value) -> Row {
        Row {
            message_data: json!({"role": role, "time": {"created": 1}}).to_string(),
            part_id: part_id.into(),
            part_time_ms: 1_793_000_000_000,
            part_data: part.to_string(),
        }
    }

    /// Why: this is the conversation view. The measured part types must come out as the right kinds, in order, with step markers dropped.
    #[test]
    fn a_turn_becomes_user_thinking_tool_assistant() {
        let rows = [
            row("user", "p1", json!({"type": "text", "text": "list the files"})),
            row("assistant", "p2", json!({"type": "step-start"})),
            row("assistant", "p3", json!({"type": "reasoning", "text": "they want ls"})),
            row(
                "assistant",
                "p4",
                json!({"type": "tool", "tool": "bash", "callID": "c1",
                       "state": {"status": "completed", "input": {"command": "ls"}, "output": "a\nb\n"}}),
            ),
            row("assistant", "p5", json!({"type": "text", "text": "Two files."})),
            row("assistant", "p6", json!({"type": "step-finish", "reason": "stop"})),
        ];
        let items = items_from_rows(&rows);
        let kinds: Vec<&ItemKind> = items.iter().map(|i| &i.kind).collect();
        assert_eq!(items.len(), 4, "{kinds:?}");
        assert_eq!(items[0].kind, ItemKind::User("list the files".into()));
        assert_eq!(items[1].kind, ItemKind::Thinking("they want ls".into()));
        match &items[2].kind {
            ItemKind::ToolCall { call_id, name, input, result } => {
                assert_eq!((call_id.as_str(), name.as_str()), ("c1", "bash"));
                assert_eq!(input, &json!({"command": "ls"}));
                let r = result.as_ref().unwrap();
                assert_eq!((r.text.as_str(), r.is_error), ("a\nb\n", false));
            }
            k => panic!("{k:?}"),
        }
        assert_eq!(items[3].kind, ItemKind::Assistant("Two files.".into()));
        assert_eq!(items[0].id, "p1");
        assert!(items[0].time.as_deref().unwrap().ends_with('Z'));
    }

    /// Why: a tool that is still running has no output yet; showing an empty result as "done" would be wrong.
    #[test]
    fn running_tool_has_no_result_and_failed_tool_is_an_error() {
        let running = row("assistant", "p1", json!({"type": "tool", "tool": "bash", "callID": "c", "state": {"status": "running", "input": {}}}));
        let failed = row("assistant", "p2", json!({"type": "tool", "tool": "bash", "callID": "d", "state": {"status": "error", "input": {}, "error": "boom"}}));
        let items = items_from_rows(&[running, failed]);
        match (&items[0].kind, &items[1].kind) {
            (ItemKind::ToolCall { result: None, .. }, ItemKind::ToolCall { result: Some(r), .. }) => assert!(r.is_error && r.text == "boom"),
            k => panic!("{k:?}"),
        }
    }

    #[test]
    fn synthetic_empty_and_unknown_parts_are_skipped_and_bad_json_does_not_panic() {
        let rows = [
            row("user", "a", json!({"type": "text", "text": "injected", "synthetic": true})),
            row("user", "b", json!({"type": "text", "text": "   "})),
            row("assistant", "c", json!({"type": "snapshot", "snapshot": "x"})),
            Row { message_data: "not json".into(), part_id: "d".into(), part_time_ms: 0, part_data: "{}".into() },
            row("assistant", "e", json!({"type": "something-new"})),
            row("user", "f", json!({"type": "text", "text": "real"})),
        ];
        let items = items_from_rows(&rows);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind, ItemKind::User("real".into()));
    }

    #[test]
    fn a_failed_assistant_message_shows_its_error_once() {
        let msg = json!({"role": "assistant", "error": {"name": "APIError", "data": {"message": "rate limited"}}}).to_string();
        let mk = |id: &str, part: Value| Row { message_data: msg.clone(), part_id: id.into(), part_time_ms: 1, part_data: part.to_string() };
        let items = items_from_rows(&[mk("a", json!({"type": "step-start"})), mk("b", json!({"type": "text", "text": "partial"}))]);
        let errors = items.iter().filter(|i| matches!(&i.kind, ItemKind::Notice { level: Level::Error, text } if text == "rate limited")).count();
        assert_eq!(errors, 1, "{items:?}");
    }

    /// Why: the view applies `Changes` to what it already shows. Appended / updated / reset must mean what the reader contract says.
    #[test]
    fn diff_reports_appended_updated_and_reset() {
        let it = |id: &str, t: &str| Item { id: id.into(), time: None, kind: ItemKind::Assistant(t.into()) };
        let old = vec![it("1", "a"), it("2", "b")];
        assert_eq!(diff(&old, &old), Changes { appended: 2..2, updated: vec![], reset: false });
        let grown = vec![it("1", "a"), it("2", "b more"), it("3", "c")];
        assert_eq!(diff(&old, &grown), Changes { appended: 2..3, updated: vec![1], reset: false });
        let shorter = vec![it("1", "a")];
        assert_eq!(diff(&old, &shorter), Changes { appended: 0..1, updated: vec![], reset: true });
    }

    /// Real database round trip: rows read in the order the view needs, and an unchanged session costs no re-read.
    #[test]
    fn source_polls_a_real_database_incrementally() {
        let dir = std::env::temp_dir().join(format!("makit-oc-tr-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("opencode.db");
        let w = Connection::open(&path).unwrap();
        w.execute_batch(
            "CREATE TABLE message (id text, session_id text, time_created integer, time_updated integer, data text);
             CREATE TABLE part (id text, message_id text, session_id text, time_created integer, time_updated integer, data text);",
        )
        .unwrap();
        let add = |mid: &str, role: &str, t: i64, pid: &str, part: Value| {
            w.execute("INSERT OR IGNORE INTO message VALUES (?1, 's', ?2, ?2, ?3)", rusqlite::params![mid, t, json!({"role": role}).to_string()]).unwrap();
            w.execute("INSERT INTO part VALUES (?1, ?2, 's', ?3, ?3, ?4)", rusqlite::params![pid, mid, t, part.to_string()]).unwrap();
        };
        add("m1", "user", 10, "p1", json!({"type": "text", "text": "hi"}));
        let mut src = OpencodeSource::new(path.clone(), "s".into());
        assert_eq!(src.poll().unwrap(), Changes { appended: 0..1, updated: vec![], reset: false });
        assert_eq!(src.poll().unwrap(), Changes { appended: 1..1, updated: vec![], reset: false }, "nothing changed");
        add("m2", "assistant", 20, "p2", json!({"type": "text", "text": "hello"}));
        assert_eq!(src.poll().unwrap(), Changes { appended: 1..2, updated: vec![], reset: false });
        assert_eq!(src.items[1].kind, ItemKind::Assistant("hello".into()));
        drop(w);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Real-data smoke test (needs opencode on this machine): `cargo test transcript::opencode::tests::real -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_sessions_parse() {
        let db = crate::opencode::db_path().unwrap();
        let conn = crate::opencode::open_read_only(&db).unwrap();
        let ids: Vec<String> = conn.prepare("SELECT id FROM session").unwrap().query_map([], |r| r.get(0)).unwrap().flatten().collect();
        for id in ids {
            let items = items_from_rows(&read_rows(&conn, &id).unwrap());
            let mut counts = std::collections::BTreeMap::new();
            for i in &items {
                *counts.entry(format!("{:?}", i.kind).split(|c: char| !c.is_alphanumeric()).next().unwrap().to_string()).or_insert(0) += 1;
            }
            println!("{id}: {} items {counts:?}", items.len());
        }
    }
}
