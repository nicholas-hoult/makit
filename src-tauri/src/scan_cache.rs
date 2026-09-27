//! 会话 jsonl 的增量扫描游标（#216）：按 inode + offset 续读，落盘到 `scan-cache.json`。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use crate::sessions::{extract_text, is_real_user_msg};

/// parse_session 逐行累加的状态。jsonl 只追加，所以这份状态可以从上次读到的位置接着累加（#216）。
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
        // claude code 写入的真实标题。多次重命名取最后一次。
        // type:"custom-title" {customTitle: "..."}  / type:"agent-name" {agentName: "..."}
        //
        // 这里**故意不读** `slug`。claude 的 slug 是 `clever-swimming-flute` 这种随机三词
        // 代号，只写在 `type:"system", subtype:"compact_boundary"` 记录上（实测 8 个带
        // slug 的会话 8 个都是这个来源），和会话内容没有任何关系。它一旦进了 display_name，
        // 前端的 `display_name || first_user_msg` 就会优先它，结果是**会话被 compact 一次，
        // 标题就从首条消息退化成随机词组**。没有真标题时留空更好：交给前端兜到首条消息。
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

/// 每个 jsonl 读到哪了：inode 变了（被替换）或文件比已读位置短（被截断）就从头来。
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

/// 缓存落盘（#216）：冷启动不用再把全部会话从头解析一遍（实测 2.8s），只读关闭之后追加的部分。
/// 位置和状态是同一条记录里一起存的，所以落盘时机早晚只影响「要补读多少」，不影响对错。
pub fn scan_cache_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude").join("makit").join("scan-cache.json"))
}

pub const SCAN_CACHE_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
pub struct ScanCacheFile {
    version: u32,
    entries: HashMap<PathBuf, ScanCursor>,
}

/// 读不出来（不存在、坏了、版本不对）一律当空缓存：代价只是从头解析一遍
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
        cache.retain(|p, _| p.exists()); // 删掉的会话不留在缓存里
        serde_json::to_vec(&ScanCacheFileRef { version: SCAN_CACHE_VERSION, entries: &cache })
            .map_err(|e| e.to_string())?
    };
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    // 先写临时文件再改名：写到一半被杀也不会留下半个文件
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json).map_err(|e| e.to_string())?;
    fs::rename(&tmp, path).map_err(|e| e.to_string())
}

#[derive(Serialize)]
pub struct ScanCacheFileRef<'a> {
    version: u32,
    entries: &'a HashMap<PathBuf, ScanCursor>,
}

/// 会话输出时每秒多一次增量扫描，没必要每次都写盘：最多 30 秒写一次
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
            eprintln!("scan-cache 写盘失败: {e}");
        }
    }
}

/// 增量扫描一个 claude 会话 jsonl（#216）。只消费以换行结尾的完整行 ——
/// claude 可能正写到一半，半行留到下次，否则会被当成坏行丢掉、之后再也不读。
pub fn scan_session_file(path: &Path) -> Option<ScanState> {
    use std::io::{Seek, SeekFrom};
    use std::os::unix::fs::MetadataExt;

    let file = fs::File::open(path).ok()?;
    let md = file.metadata().ok()?;
    let (ino, len) = (md.ino(), md.len());

    // 锁只在取出 / 放回时持有，读文件期间不持锁
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
            break; // 半行：不前进
        }
        offset += n as u64;
        // 和原来 reader.lines() 一样：非法 UTF-8 的行跳过
        if let Ok(line) = std::str::from_utf8(&buf[..n - 1]) {
            state.feed_line(line.strip_suffix('\r').unwrap_or(line));
        }
    }

    if let Ok(mut cache) = SCAN_CACHE.lock() {
        cache.insert(path.to_path_buf(), ScanCursor { ino, offset, state: state.clone() });
    }
    Some(state)
}

/// #216：会话输出时每秒多一次 `list_sessions_by_paths`，以前每次把整个 jsonl
/// （本机最大 43MB）重新解析一遍，0.2–0.4s CPU。现在记住读到的字节位置，只读追加部分。
/// 错了在 UI 上：侧栏的消息数 / 首末条消息 / 标题和 ⌘R 全量刷新后的不一致，
/// 或者 claude 正写到一半的那行被吞掉、被数两次。
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

    /// 启动时（新进程，内存缓存是空的）以前要把全部会话文件从头解析一遍：实测冷启动 2.8s。
    /// 缓存落盘后，重启只读上次关闭之后追加的部分。错了在 UI 上：重启后侧栏的消息数 /
    /// 首末条 / 标题和实际不符（缓存和文件对不上还被信任），或缓存文件坏了导致侧栏空白。
    #[test]
    fn persisted_cursor_survives_restart_and_resumes_from_offset() {
        let p = tmpfile("persist");
        let _ = fs::remove_file(&p);
        append(&p, &user("一", "/a"));
        append(&p, &user("二", "/a"));
        scan_session_file(&p).unwrap();
        let cache_file = p.with_file_name("scan-cache.json");
        save_scan_cache_to(&cache_file).unwrap();

        // 模拟重启：内存缓存清空，只从磁盘恢复
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
        // 同路径换成更短的新文件（截断）
        fs::write(&p, user("新", "/n")).unwrap();
        let s = scan_session_file(&p).unwrap();
        assert_eq!((s.user_count, s.first_msg.as_str()), (1, "新"));
        // 原子替换（新 inode），长度比已读位置还长
        let tmp = p.with_extension("tmp");
        fs::write(&tmp, format!("{}{}{}", user("替一", "/r"), user("替二", "/r"), user("替三", "/r"))).unwrap();
        fs::rename(&tmp, &p).unwrap();
        let s = scan_session_file(&p).unwrap();
        assert_eq!((s.user_count, s.first_msg.as_str()), (3, "替一"));
    }
}
