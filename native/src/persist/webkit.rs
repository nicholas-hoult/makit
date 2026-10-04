//! 从 Tauri 版的 WebKit localStorage 导入（只读）。
//!
//! 文件：`~/Library/WebKit/com.hoult.makit/WebsiteData/Default/<hash>/<hash>/LocalStorage/localstorage.sqlite3`，
//! 表 `ItemTable(key TEXT, value BLOB)`，值是 UTF-16LE。
//!
//! 不引 sqlite 库：用系统自带的 `/usr/bin/sqlite3 -readonly` 读 `hex(value)`（macOS 一定有，
//! Tauri 版正开着、库是 WAL 模式也能读）。读不到 / 没这个文件一律返回 None，调用方按默认状态启动。

use std::path::{Path, PathBuf};
use std::process::Command;

use super::state::{from_local_storage, parse_sqlite_hex_dump, NativeState};

/// Tauri 版的 WebKit 数据目录（identifier 是 com.hoult.makit）
pub fn default_root() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join("Library").join("WebKit").join("com.hoult.makit"))
}

/// root 下所有 localstorage.sqlite3 里最近修改的那个
pub fn find_localstorage(root: &Path) -> Option<PathBuf> {
    find_localstorage_with_mtime(root).map(|(_, f)| f)
}

/// 同 `find_localstorage`，另外给出它最后一次写的时间（含 -wal）
pub fn find_localstorage_with_mtime(root: &Path) -> Option<(std::time::SystemTime, PathBuf)> {
    let default = root.join("WebsiteData").join("Default");
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for a in std::fs::read_dir(&default).ok()?.flatten() {
        for b in std::fs::read_dir(a.path()).into_iter().flatten().flatten() {
            let f = b.path().join("LocalStorage").join("localstorage.sqlite3");
            let Ok(md) = std::fs::metadata(&f) else { continue };
            // WAL 模式下最新的写在 -wal 里，按两者较新的算
            let wal = std::fs::metadata(f.with_extension("sqlite3-wal")).and_then(|m| m.modified()).ok();
            let t = md.modified().ok().into_iter().chain(wal).max()?;
            if best.as_ref().is_none_or(|(bt, _)| t > *bt) {
                best = Some((t, f));
            }
        }
    }
    best
}

/// 只读导出 `makit-*` 键值
/// 读库。库是 WAL 模式：Tauri 退出后 -shm 被清掉，`-readonly` 打不开（要建 -shm）；直接读又会在 Tauri 的
/// 目录里留下 -shm。所以连同 -wal / -shm 拷到临时目录读副本（Tauri 开着时 -wal 里的最新写也带上），原库不碰
pub fn read_localstorage(db: &Path) -> Option<std::collections::BTreeMap<String, String>> {
    let dir = std::env::temp_dir().join(format!("makit-ls-{}-{:?}", std::process::id(), std::thread::current().id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).ok()?;
    let copy = dir.join("localstorage.sqlite3");
    let out = (|| {
        std::fs::copy(db, &copy).ok()?;
        for ext in ["sqlite3-wal", "sqlite3-shm"] {
            let _ = std::fs::copy(db.with_extension(ext), copy.with_extension(ext));
        }
        Command::new("/usr/bin/sqlite3").arg(&copy).arg("select key, hex(value) from ItemTable").output().ok()
    })();
    let _ = std::fs::remove_dir_all(&dir);
    let out = out?;
    if !out.status.success() {
        log::warn!(target: "persist", "reading {} failed: {}", db.display(), String::from_utf8_lossy(&out.stderr).trim());
        return None;
    }
    Some(parse_sqlite_hex_dump(&String::from_utf8_lossy(&out.stdout)))
}

/// 找到并导入。返回 (状态, 来源文件路径)
pub fn import_from(root: &Path) -> Option<(NativeState, String)> {
    let db = find_localstorage(root)?;
    let ls = read_localstorage(&db)?;
    if ls.is_empty() {
        return None;
    }
    let mut s = from_local_storage(&ls);
    let from = db.to_string_lossy().into_owned();
    s.imported_from = Some(from.clone());
    Some((s, from))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// 造一个和 WebKit 同结构的 localStorage 库（值按 UTF-16LE 存）
    pub fn make_localstorage(root: &Path, items: &[(&str, &str)]) -> PathBuf {
        let dir = root.join("WebsiteData/Default/h1/h1/LocalStorage");
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("localstorage.sqlite3");
        let _ = std::fs::remove_file(&db);
        let mut sql = String::from("CREATE TABLE ItemTable (key TEXT UNIQUE ON CONFLICT REPLACE, value BLOB NOT NULL ON CONFLICT FAIL);");
        for (k, v) in items {
            let hex: String = v.encode_utf16().flat_map(|u| u.to_le_bytes()).map(|b| format!("{b:02X}")).collect();
            sql += &format!("INSERT INTO ItemTable VALUES('{k}', X'{hex}');");
        }
        let st = Command::new("/usr/bin/sqlite3").arg(&db).arg(&sql).status().unwrap();
        assert!(st.success());
        db
    }

    /// WebKit 的库是 WAL 模式；Tauri 退出后 -shm 被清掉，`sqlite3 -readonly` 直接报 unable to open（14）。
    /// 错了就是「Tauri 关着时 GPUI 启动，导入静默失败，恢复不出 Tauri 里的标签」
    #[test]
    fn reads_wal_mode_db_after_tauri_quit() {
        let root = std::env::temp_dir().join(format!("makit-webkit-wal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let db = make_localstorage(&root, &[("makit-theme", "nord")]);
        let st = Command::new("/usr/bin/sqlite3").arg(&db).arg("pragma journal_mode=wal;").output().unwrap();
        assert!(String::from_utf8_lossy(&st.stdout).contains("wal"));
        assert!(!db.with_extension("sqlite3-shm").exists(), "干净关闭后 -shm 不在（复现 Tauri 退出后的样子）");
        let before = std::fs::read(&db).unwrap();
        let ls = read_localstorage(&db).expect("WAL 且没有 -shm 也要读得出");
        assert_eq!(ls.get("makit-theme").map(String::as_str), Some("nord"));
        assert_eq!(std::fs::read(&db).unwrap(), before, "原库一个字节都不动");
        assert!(!db.with_extension("sqlite3-shm").exists(), "不在 Tauri 的目录里留 -shm");
    }

    #[test]
    fn reads_real_webkit_layout_read_only() {
        let root = std::env::temp_dir().join(format!("makit-webkit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let db = make_localstorage(&root, &[("makit-theme", "tokyo-night"), ("makit-section-collapsed", "[\"最近\"]"), ("other", "x")]);
        std::fs::create_dir_all(root.join("WebsiteData/Default/h2/h2")).unwrap(); // 没有 LocalStorage 的兄弟目录
        let before = std::fs::read(&db).unwrap();
        assert_eq!(find_localstorage(&root), Some(db.clone()));
        let (s, from) = import_from(&root).unwrap();
        assert_eq!(s.theme.id, "tokyo-night");
        assert_eq!(s.sidebar.section_collapsed, ["最近"]);
        assert_eq!(from, db.to_string_lossy());
        assert_eq!(std::fs::read(&db).unwrap(), before, "只读：库文件一个字节都不能变");
        assert!(import_from(&root.join("nope")).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }
}
