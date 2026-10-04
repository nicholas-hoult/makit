//! Incremental scan cursor for session jsonl (#216): resumes reading by inode + offset, persisted to `scan-cache.json`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use crate::sessions::{extract_text, is_real_user_msg};

/// State that parse_session accumulates line by line. A jsonl is append-only, so this state can keep accumulating from the last read position (#216).
#[derive(Clone, Default, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScanState {
    pub start_cwd: String,
    pub last_cwd: String,
    pub git_branch: String,
    pub user_count: u32,
    pub first_msg: String,
    pub last_msg: String,
    pub custom_title: String,
    pub agent_name: String,
}

impl ScanState {
    fn feed_line(&mut self, line: &str) {
        if line.trim().is_empty() {
            return;
        }
        let rec: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => return,
        };
        // The real title written by claude code. With multiple renames, the last one wins.
        // type:"custom-title" {customTitle: "..."}  / type:"agent-name" {agentName: "..."}
        //
        // `slug` is **deliberately not read** here. claude's slug is a random three-word
        // codename like `clever-swimming-flute`, written only on `type:"system", subtype:"compact_boundary"` records (measured: of 8 sessions
        // with a slug, all 8 came from this source), and has nothing to do with the session content. Once it got into display_name,
        // the frontend's `display_name || first_user_msg` would prefer it, with the result that **after a session is compacted once,
        // its title degrades from the first message to a random phrase**. With no real title, leaving it empty is better: the frontend falls back to the first message.
        let rec_type = rec.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if rec_type == "custom-title" {
            if let Some(s) = rec.get("customTitle").and_then(|v| v.as_str()) {
                if !s.is_empty() {
                    self.custom_title = s.to_string();
                }
            }
            return;
        }
        if rec_type == "agent-name" {
            if let Some(s) = rec.get("agentName").and_then(|v| v.as_str()) {
                if !s.is_empty() {
                    self.agent_name = s.to_string();
                }
            }
            return;
        }
        if rec_type != "user" {
            return;
        }
        if rec.get("isSidechain").and_then(|v| v.as_bool()) == Some(true) {
            return;
        }
        if let Some(c) = rec.get("cwd").and_then(|v| v.as_str()) {
            if !c.is_empty() {
                if self.start_cwd.is_empty() {
                    self.start_cwd = c.to_string();
                }
                self.last_cwd = c.to_string();
            }
        }
        if let Some(gb) = rec.get("gitBranch").and_then(|v| v.as_str()) {
            if !gb.is_empty() && gb != "HEAD" {
                self.git_branch = gb.to_string();
            }
        }
        let content = rec.pointer("/message/content").cloned().unwrap_or(serde_json::Value::Null);
        let text = extract_text(&content);
        if !is_real_user_msg(&text) {
            return;
        }
        self.user_count += 1;
        if self.first_msg.is_empty() {
            self.first_msg = text.clone();
        }
        self.last_msg = text;
    }
}

/// How far each jsonl has been read: if the inode changed (replaced) or the file is shorter than the read position (truncated), start over.
#[derive(Serialize, Deserialize)]
pub struct ScanCursor {
    ino: u64,
    offset: u64,
    state: ScanState,
}

pub static SCAN_CACHE: std::sync::LazyLock<std::sync::Mutex<HashMap<PathBuf, ScanCursor>>> =
    std::sync::LazyLock::new(|| {
        let loaded = scan_cache_path().map(|p| load_scan_cache_from(&p)).unwrap_or_default();
        std::sync::Mutex::new(loaded)
    });

/// Cache persistence (#216): a cold start no longer has to re-parse every session from scratch (measured 2.8s); it reads only what was appended after the last shutdown.
/// The position and the state are stored together in one record, so when it is persisted only affects "how much has to be re-read", not correctness.
pub fn scan_cache_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude").join("makit").join("scan-cache.json"))
}

pub const SCAN_CACHE_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
pub struct ScanCacheFile {
    version: u32,
    entries: HashMap<PathBuf, ScanCursor>,
}

/// Anything unreadable (missing, corrupt, wrong version) is treated as an empty cache: the only cost is a parse from scratch
pub fn load_scan_cache_from(path: &Path) -> HashMap<PathBuf, ScanCursor> {
    fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice::<ScanCacheFile>(&b).ok())
        .filter(|f| f.version == SCAN_CACHE_VERSION)
        .map(|f| f.entries)
        .unwrap_or_default()
}

pub fn save_scan_cache_to(path: &Path) -> Result<(), String> {
    let json = {
        let mut cache = SCAN_CACHE.lock().map_err(|e| e.to_string())?;
        cache.retain(|p, _| p.exists()); // sessions that were deleted do not stay in the cache
        serde_json::to_vec(&ScanCacheFileRef { version: SCAN_CACHE_VERSION, entries: &cache })
            .map_err(|e| e.to_string())?
    };
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    // Write a temp file first, then rename: being killed halfway never leaves a half-written file.
    // The temp file name carries the pid and a sequence number: several makit instances can run on one machine (packaged + dev), and several threads in one process can save at once;
    // with a shared temp file, after the first rename the second cannot find the file (the "write failed: No such file or directory" in the log)
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let tmp = path.with_extension(format!("json.{}.{}.tmp", std::process::id(), SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    let result = fs::write(&tmp, json).and_then(|_| fs::rename(&tmp, path));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result.map_err(|e| e.to_string())
}

#[derive(Serialize)]
pub struct ScanCacheFileRef<'a> {
    version: u32,
    entries: &'a HashMap<PathBuf, ScanCursor>,
}

/// While a session is producing output there is an extra incremental scan every second; no need to write to disk every time: at most once per 30 seconds
pub fn save_scan_cache_throttled(force: bool) {
    static LAST: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
    let Ok(mut last) = LAST.lock() else { return };
    if !force && last.is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(30)) {
        return;
    }
    *last = Some(std::time::Instant::now());
    drop(last);
    if let Some(p) = scan_cache_path() {
        if let Err(e) = save_scan_cache_to(&p) {
            log::warn!(target: "scan_cache", "写盘失败: {e}");
        }
    }
}

/// Incrementally scan a claude session jsonl (#216). Only complete lines ending in a newline are consumed --
/// claude may be midway through writing; a half line is left for next time, otherwise it would be taken for a bad line, dropped, and never read again.
pub fn scan_session_file(path: &Path) -> Option<ScanState> {
    use std::io::{Seek, SeekFrom};
    use std::os::unix::fs::MetadataExt;

    let file = fs::File::open(path).ok()?;
    let md = file.metadata().ok()?;
    let (ino, len) = (md.ino(), md.len());

    // The lock is held only while taking out / putting back, not while reading the file
    let cached = SCAN_CACHE.lock().ok()?.remove(path);
    let (mut offset, mut state) = match cached {
        Some(c) if c.ino == ino && c.offset <= len => (c.offset, c.state),
        _ => (0, ScanState::default()),
    };

    let mut reader = BufReader::new(file);
    reader.seek(SeekFrom::Start(offset)).ok()?;
    let mut buf: Vec<u8> = Vec::new();
    loop {
        buf.clear();
        let n = match reader.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        if buf.last() != Some(&b'\n') {
            break; // half line: do not advance
        }
        offset += n as u64;
        // Same as the original reader.lines(): lines with invalid UTF-8 are skipped
        if let Ok(line) = std::str::from_utf8(&buf[..n - 1]) {
            state.feed_line(line.strip_suffix('\r').unwrap_or(line));
        }
    }

    if let Ok(mut cache) = SCAN_CACHE.lock() {
        cache.insert(path.to_path_buf(), ScanCursor { ino, offset, state: state.clone() });
    }
    Some(state)
}

/// #216: while a session is producing output there is an extra `list_sessions_by_paths` every second, and it used to re-parse the whole jsonl
/// (the largest here is 43MB) each time, costing 0.2-0.4s of CPU. Now the byte position already read is remembered and only the appended part is read.
/// What goes wrong on screen if this breaks: the sidebar's message count / first and last message / title disagree with what a full refresh shows,
/// or the line claude is midway through writing gets swallowed or counted twice.
#[cfg(test)]
mod save_race_tests {
    use super::*;

    /// Why this test exists: two makit instances (or two threads) saving at once used to share one
    /// `scan-cache.json.tmp`; the first rename moved it away and the second failed with "No such file or
    /// directory", which showed up in the log as a "scan_cache: write failed" warning.
    #[test]
    fn concurrent_saves_to_the_same_file_all_succeed() {
        let dir = std::env::temp_dir().join(format!("makit-scan-race-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("scan-cache.json");
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let path = path.clone();
                std::thread::spawn(move || (0..100).map(|_| save_scan_cache_to(&path)).filter_map(Result::err).collect::<Vec<_>>())
            })
            .collect();
        let errors: Vec<String> = handles.into_iter().flat_map(|h| h.join().unwrap()).collect();
        assert!(errors.is_empty(), "concurrent saves failed: {:?}", &errors[..errors.len().min(3)]);
        assert!(path.exists());
        let leftovers = fs::read_dir(&dir).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().ends_with(".tmp")).count();
        assert_eq!(leftovers, 0, "temp files must not be left behind");
        let _ = fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod incremental_scan_tests {
    use super::*;
    use std::io::Write;

    fn user(text: &str, cwd: &str) -> String {
        format!("{{\"type\":\"user\",\"cwd\":\"{cwd}\",\"gitBranch\":\"main\",\"message\":{{\"role\":\"user\",\"content\":\"{text}\"}}}}\n")
    }
    fn tmpfile(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("makit-scan-{}-{name}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        dir.join("s.jsonl")
    }
    fn append(p: &Path, s: &str) {
        fs::OpenOptions::new().create(true).append(true).open(p).unwrap().write_all(s.as_bytes()).unwrap();
    }
    fn full(p: &Path) -> ScanState {
        let mut st = ScanState::default();
        for l in fs::read_to_string(p).unwrap().lines() { st.feed_line(l); }
        st
    }

    #[test]
    fn appended_lines_match_full_rescan() {
        let p = tmpfile("append");
        let _ = fs::remove_file(&p);
        append(&p, &user("第一条", "/a"));
        append(&p, "{\"type\":\"assistant\",\"message\":{\"content\":\"x\"}}\n");
        let s1 = scan_session_file(&p).unwrap();
        assert_eq!(s1.user_count, 1);
        append(&p, &user("第二条", "/b"));
        append(&p, "{\"type\":\"custom-title\",\"customTitle\":\"改名了\"}\n");
        let s2 = scan_session_file(&p).unwrap();
        assert_eq!(s2, full(&p));
        assert_eq!((s2.user_count, s2.first_msg.as_str(), s2.last_msg.as_str()), (2, "第一条", "第二条"));
        assert_eq!((s2.start_cwd.as_str(), s2.last_cwd.as_str(), s2.custom_title.as_str()), ("/a", "/b", "改名了"));
    }

    #[test]
    fn half_written_line_is_counted_once_after_it_completes() {
        let p = tmpfile("partial");
        let _ = fs::remove_file(&p);
        append(&p, &user("一", "/a"));
        let line = user("二", "/a");
        let (head, tail) = line.split_at(20);
        append(&p, head);
        assert_eq!(scan_session_file(&p).unwrap().user_count, 1, "半行不能算");
        append(&p, tail);
        assert_eq!(scan_session_file(&p).unwrap().user_count, 2, "写完之后算且只算一次");
        assert_eq!(scan_session_file(&p).unwrap().user_count, 2, "没新内容时不变");
    }

    /// At startup (a new process, with an empty in-memory cache) every session file used to be re-parsed from scratch: measured cold start 2.8s.
    /// With the cache persisted, a restart reads only what was appended after the last shutdown. What goes wrong on screen if this breaks: after a restart the sidebar's message count /
    /// first and last message / title do not match reality (a cache that disagrees with the file is still trusted), or a corrupt cache file leaves the sidebar blank.
    #[test]
    fn persisted_cursor_survives_restart_and_resumes_from_offset() {
        let p = tmpfile("persist");
        let _ = fs::remove_file(&p);
        append(&p, &user("一", "/a"));
        append(&p, &user("二", "/a"));
        scan_session_file(&p).unwrap();
        let cache_file = p.with_file_name("scan-cache.json");
        save_scan_cache_to(&cache_file).unwrap();

        // simulate a restart: clear the in-memory cache and restore from disk only
        SCAN_CACHE.lock().unwrap().remove(&p);
        let loaded = load_scan_cache_from(&cache_file);
        let len_before = fs::metadata(&p).unwrap().len();
        assert_eq!(loaded.get(&p).map(|c| c.offset), Some(len_before), "恢复出来的位置 = 关闭时读到的位置");
        SCAN_CACHE.lock().unwrap().extend(loaded);

        append(&p, &user("三", "/b"));
        let s = scan_session_file(&p).unwrap();
        assert_eq!(s, full(&p));
        assert_eq!((s.user_count, s.last_msg.as_str()), (3, "三"));
    }

    #[test]
    fn corrupt_or_missing_cache_file_is_ignored() {
        let dir = tmpfile("corrupt");
        let cache_file = dir.with_file_name("scan-cache.json");
        fs::write(&cache_file, "{ 这不是 json").unwrap();
        assert!(load_scan_cache_from(&cache_file).is_empty());
        assert!(load_scan_cache_from(&dir.with_file_name("不存在.json")).is_empty());
    }

    #[test]
    fn replaced_or_truncated_file_is_rescanned_from_start() {
        let p = tmpfile("replace");
        let _ = fs::remove_file(&p);
        append(&p, &user("旧一", "/a"));
        append(&p, &user("旧二", "/a"));
        assert_eq!(scan_session_file(&p).unwrap().user_count, 2);
        // replace with a shorter new file at the same path (truncation)
        fs::write(&p, user("新", "/n")).unwrap();
        let s = scan_session_file(&p).unwrap();
        assert_eq!((s.user_count, s.first_msg.as_str()), (1, "新"));
        // atomic replacement (new inode), whose length is still longer than the read position
        let tmp = p.with_extension("tmp");
        fs::write(&tmp, format!("{}{}{}", user("替一", "/r"), user("替二", "/r"), user("替三", "/r"))).unwrap();
        fs::rename(&tmp, &p).unwrap();
        let s = scan_session_file(&p).unwrap();
        assert_eq!((s.user_count, s.first_msg.as_str()), (3, "替一"));
    }
}
