//! Recovery after a session's start directory is deleted / moved (#173): storage-key encoding, symlinks, rebuild or point to a new location.

use crate::ts;
use serde::Serialize;
use std::fs;
use std::path::Path;

use crate::paths::projects_dir;

/// claude's storage key: encodes the cwd character by character into the directory name under `~/.claude/projects/`.
///
/// **canonicalize first, then encode**. claude computes this key from its own process's cwd, and a process cwd is always
/// the real path with symlinks resolved (on macOS `/var` = `/private/var`, `/tmp` = `/private/tmp`).
/// Without resolving, the computed key is one claude will never use -- the symlink gets created in the wrong place and "resume session"
/// silently fails (hit in practice, see `session_recovery_tests`).
///
/// When the path does not exist canonicalize fails and we fall back to encoding it as-is. That is exactly the "original directory was deleted" case, and
/// falling back as-is is right there: the cwd recorded in the jsonl is itself the real path claude wrote back then.
/// Whether `err` (an error message returned by `recover_session_cwd`) says the chosen target folder does not
/// exist. The UI uses it to show a friendlier message; it works in every language because it compares with the
/// start of the message in each locale.
pub fn is_target_missing_error(err: &str) -> bool {
    ["zh", "en"].iter().any(|locale| {
        let text = rust_i18n::t!("core.recovery.target_missing", locale = *locale, dir = "\u{1}");
        let prefix = text.split('\u{1}').next().unwrap_or("");
        !prefix.is_empty() && err.starts_with(prefix)
    })
}

pub fn encode_project_path(path: &str) -> String {
    let resolved = std::fs::canonicalize(path)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_string());
    resolved
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' })
        .collect()
}

pub fn ensure_session_symlink(session_id: String, cwd: String, storage_folder: String) -> Result<String, String> {
    let dir = projects_dir().ok_or(ts!("core.err.no_home"))?;
    let encoded = encode_project_path(&cwd);
    if encoded == storage_folder {
        return Ok(ts!("core.recovery.nothing_to_do").into());
    }
    let session_file = format!("{}.jsonl", session_id);
    let target_dir = dir.join(&encoded);
    let target_file = target_dir.join(&session_file);
    if target_file.exists() {
        return Ok(ts!("core.recovery.exists").into());
    }
    let source_file = dir.join(&storage_folder).join(&session_file);
    if !source_file.exists() {
        return Err(ts!("core.recovery.source_missing", folder = storage_folder, file = session_file));
    }
    // target directory missing -> link the whole directory; already exists -> link the single file
    if !target_dir.exists() {
        let source_dir = dir.join(&storage_folder);
        link_path(&source_dir, &target_dir, true).map_err(|e| ts!("core.recovery.dir_symlink_failed", error = e))?;
    } else {
        link_path(&source_file, &target_file, false).map_err(|e| ts!("core.recovery.file_symlink_failed", error = e))?;
    }
    Ok(format!("symlink: {}/{}", encoded, session_file))
}

#[derive(Serialize, Debug)]
pub struct RecoveredSession {
    /// Which cwd to use when resuming after recovery
    pub cwd: String,
    /// What exactly was changed (we must be able to explain it clearly to the user, not just say "success")
    pub detail: String,
}

/// Recover the session after the start directory was deleted. `projects_root` is injectable so tests do not write into the real `~/.claude`.
///
/// In essence: not a byte of session data was lost, only **a key**. `claude -r <id>` only looks at
/// `projects/<encode(realpath(cwd))>/<id>.jsonl`, and once the directory is deleted you can no longer compute
/// that key. So both actions do just one thing -- **make the key match**:
///
/// - `recreate`: rebuild the original directory (empty). The key was computed from the original path in the first place, so rebuilding it matches naturally,
///   and `projects/` is not touched at all. The code is gone, but the session can continue the conversation.
/// - `relink`: the code moved, point to the new directory. Under the new key, place a
///   **file-level** symlink to the original transcript. Not directory-level: that would expose every session of the old project under the new key.
///
/// Neither mode **ever copies** the jsonl -- copying would produce two diverging copies of the same session id.
pub fn recover_session_cwd_in(
    projects_root: &Path,
    mode: &str,
    session_id: &str,
    original_cwd: &str,
    target_cwd: &str,
    storage_folder: &str,
) -> Result<RecoveredSession, String> {
    match mode {
        "recreate" => {
            // Only create the **last level**; the parent directory must already exist. `create_dir_all` would create all
            // missing parents, and "a whole parent tree is gone" is usually not "one project directory was deleted" but an external drive
            // not being mounted / the whole parent being moved away. In that case, creating a real directory at the mount point has real consequences:
            // the drive would later be mounted by macOS as "X 1", with no warning at all.
            let parent = Path::new(original_cwd).parent();
            match parent {
                Some(p) if !p.as_os_str().is_empty() && !p.is_dir() => {
                    return Err(ts!("core.recovery.parent_missing", dir = p.display()));
                }
                _ => {}
            }
            fs::create_dir_all(original_cwd).map_err(|e| ts!("core.recovery.recreate_failed", error = e))?;
            Ok(RecoveredSession {
                cwd: original_cwd.to_string(),
                detail: ts!("core.recovery.recreated", dir = original_cwd),
            })
        }
        "relink" => {
            if !Path::new(target_cwd).is_dir() {
                return Err(ts!("core.recovery.target_missing", dir = target_cwd));
            }
            // codex sessions are stored layered by **date** under `~/.codex/sessions/year/month/day/`, **not by cwd key**
            // (that is what codex's `storage_folder` being always an empty string in `ai_provider.rs` means).
            // In other words codex has no such key to lose -- `codex resume <id>` finds the session from any directory.
            // So there is no key to fix here, and "point to new location" is just switching the working directory; not a single symlink should be created.
            if storage_folder.is_empty() {
                return Ok(RecoveredSession {
                    cwd: target_cwd.to_string(),
                    detail: ts!("core.recovery.switched", dir = target_cwd),
                });
            }
            let session_file = format!("{}.jsonl", session_id);
            let source = projects_root.join(storage_folder).join(&session_file);
            // Confirm the source exists first, then act -- otherwise a dangling symlink is left behind, which is harder to track down than an error
            if !source.exists() {
                return Err(ts!("core.recovery.record_missing", folder = storage_folder, file = session_file));
            }
            let key = encode_project_path(target_cwd);
            if key == storage_folder {
                return Ok(RecoveredSession {
                    cwd: target_cwd.to_string(),
                    detail: ts!("core.recovery.already_matches").into(),
                });
            }
            let target_dir = projects_root.join(&key);
            // a real directory + file-level symlinks inside it, no directory-level symlink
            fs::create_dir_all(&target_dir).map_err(|e| ts!("core.recovery.mkdir_failed", error = e))?;
            let target_file = target_dir.join(&session_file);
            if !target_file.exists() {
                link_path(&source, &target_file, false).map_err(|e| ts!("core.recovery.symlink_failed", error = e))?;
            }
            Ok(RecoveredSession {
                cwd: target_cwd.to_string(),
                detail: ts!("core.recovery.linked", key = key),
            })
        }
        other => Err(ts!("core.recovery.unknown_mode", mode = other)),
    }
}

/// Whether the directory still exists. Deliberately **not** made into `SessionMeta.cwd_exists`: each stat in `list_sessions`
/// was measured locally at about 0.7ms, about 0.5s in total across 300+ files, already an existing performance pain point (#144),
/// and adding one more stat per session means adding cost to a known hot path. This information is only needed at "the moment the user opens a session",
/// when a single stat is enough.
pub fn dir_exists(path: String) -> bool {
    !path.is_empty() && Path::new(&path).is_dir()
}

pub fn recover_session_cwd(
    mode: String,
    session_id: String,
    original_cwd: String,
    target_cwd: String,
    storage_folder: String,
) -> Result<RecoveredSession, String> {
    let root = projects_dir().ok_or(ts!("core.err.no_home"))?;
    recover_session_cwd_in(&root, &mode, &session_id, &original_cwd, &target_cwd, &storage_folder)
}

/// #173 "how to recover when a session's start directory was deleted".
///
/// In essence: not a byte of session data was lost, only **a key**. The only way `claude -r <id>` finds a session
/// is to look in `~/.claude/projects/<encode(realpath(cwd))>/<id>.jsonl` -- when the directory is deleted
/// and you launch elsewhere, the computed key differs, claude reports "No conversation found", while the transcript
/// still lies faithfully in the original storage directory. So "resume session" = **make the key match**, not recover data; any
/// approach that copies the jsonl produces two diverging copies of the same session id, and is the wrong direction.
///
/// Measured (isolated sandbox, `CLAUDE_CONFIG_DIR` pointing at a temp directory, the real `~/.claude` untouched):
///   - real id, key mismatch -> `No conversation found with session ID`
///   - the same id, **a brand-new empty directory** + the transcript placed under that directory's encoded key -> found
///     (the error becomes "Provide a prompt to continue")
/// That is, the **original directory need not exist at all**; any existing directory will do.
/// Makes `dst` point at `src`. unix: a symlink. Windows (platform matrix P4): symlinks need a privilege, so a directory
/// gets a junction (no privilege needed) and a file gets a symlink, falling back to a copy
fn link_path(src: &std::path::Path, dst: &std::path::Path, is_dir: bool) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let _ = is_dir;
        std::os::unix::fs::symlink(src, dst)
    }
    #[cfg(windows)]
    {
        if is_dir {
            let st = std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(dst).arg(src).output()?;
            if st.status.success() {
                Ok(())
            } else {
                Err(std::io::Error::other(String::from_utf8_lossy(&st.stdout).trim().to_string()))
            }
        } else {
            std::os::windows::fs::symlink_file(src, dst).or_else(|_| std::fs::copy(src, dst).map(|_| ()))
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (src, dst, is_dir);
        Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "links are not supported on this platform"))
    }
}

#[cfg(test)]
mod session_recovery_tests {
    /// Regression: the storage key **must be canonicalized before encoding**.
    ///
    /// claude computes this key from its own process's cwd, and a process cwd is always the real path with symlinks resolved --
    /// on macOS `/var` is a symlink to `/private/var` and `/tmp` to `/private/tmp`. Without canonicalizing,
    /// a path containing symlinks gets a key claude will never use, the symlink is created in the wrong place,
    /// and "resume" silently fails.
    ///
    /// This pitfall was found by experiment: the first time the recovery experiment was run, the hand-made key used `/var/folders/...`,
    /// while claude computes `/private/var/folders/...`, so even though the transcript was right there it still reported
    /// "No conversation found"; a whole round was wasted before discovering the mechanism was fine and the key was miscomputed.
    #[test]
    fn encode_canonicalizes_before_encoding() {
        let raw = std::env::temp_dir().join(format!("makit-enc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&raw);
        std::fs::create_dir_all(&raw).unwrap();
        let raw_str = raw.to_string_lossy().into_owned();
        let canonical = std::fs::canonicalize(&raw).unwrap().to_string_lossy().into_owned();

        // Precondition: the temp directory here really goes through a symlink, otherwise this test tests nothing
        assert_ne!(
            raw_str, canonical,
            "本机 temp_dir 不含软链（{raw_str}），这条测试在这台机器上无效，需换一个含软链的路径"
        );

        assert_eq!(
            super::encode_project_path(&raw_str),
            super::encode_project_path(&canonical),
            "软链路径和实路径必须编码成同一个键（claude 用的是实路径那个）"
        );

        let _ = std::fs::remove_dir_all(&raw);
    }

    /// When the path does not exist canonicalize fails, and we must fall back to encoding character by character as-is -- which is exactly
    /// the "original directory was deleted" case. Falling back as-is is **correct** there: the cwd recorded in the jsonl is itself
    /// the real path claude wrote back then, and need not (and cannot) be resolved again.
    #[test]
    fn encode_falls_back_to_raw_when_path_is_gone() {
        assert_eq!(
            super::encode_project_path("/definitely/not/a/path/ai-claw.studio"),
            "-definitely-not-a-path-ai-claw-studio"
        );
    }

    const SID: &str = "4110cea1-8771-4e89-b521-b93f5a677c5a";

    /// Build a fake `projects/` root + a storage directory holding the transcript.
    /// Never touch the real `~/.claude`: all recovery logic is funneled into `*_in(projects_root, ...)`,
    /// precisely so tests can point elsewhere.
    fn sandbox(tag: &str) -> (std::path::PathBuf, String) {
        let root = std::env::temp_dir().join(format!("makit-recover-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let projects = root.join("projects");
        // The original cwd is deliberately placed under root and **not created**: simulates "the start directory was deleted"
        let original_cwd = root.join("gone-project").to_string_lossy().into_owned();
        let storage = super::encode_project_path(&original_cwd);
        std::fs::create_dir_all(projects.join(&storage)).unwrap();
        std::fs::write(
            projects.join(&storage).join(format!("{SID}.jsonl")),
            "{\"cwd\":\"x\",\"type\":\"user\"}\n",
        )
        .unwrap();
        (root, original_cwd)
    }

    /// Action A "rebuild the original directory": the key was computed from the original path, so building the empty directory back makes the key match immediately.
    /// The key assertion is that **not a single symlink may be created** -- of the three recovery paths this is the only one that does not touch
    /// claude's data directory at all, and its value lies exactly there.
    #[test]
    fn recreate_matches_the_key_without_touching_claude_dir() {
        let (root, original_cwd) = sandbox("recreate");
        let projects = root.join("projects");
        let before: Vec<_> = std::fs::read_dir(&projects).unwrap().flatten().map(|e| e.file_name()).collect();

        let r = super::recover_session_cwd_in(&projects, "recreate", SID, &original_cwd, "", "").unwrap();

        assert!(std::path::Path::new(&r.cwd).is_dir(), "原目录必须被建回来");
        assert_eq!(r.cwd, original_cwd, "重建之后就该在原路径上 resume");
        let after: Vec<_> = std::fs::read_dir(&projects).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(before, after, "重建原目录这条路不该在 projects/ 下留下任何东西");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Action B "point to new location": the code moved and the session must continue in the new directory.
    /// Use a **file-level** symlink rather than a directory-level one: a directory-level one would expose every session of the old project under the new key,
    /// and the enumeration in `list_sessions` would step on it.
    #[test]
    fn relink_makes_a_brand_new_dir_resumable() {
        let (root, original_cwd) = sandbox("relink");
        let projects = root.join("projects");
        let storage = super::encode_project_path(&original_cwd);
        let new_dir = root.join("moved-here");
        std::fs::create_dir_all(&new_dir).unwrap();
        let new_dir_s = new_dir.to_string_lossy().into_owned();

        let r = super::recover_session_cwd_in(&projects, "relink", SID, &original_cwd, &new_dir_s, &storage)
            .unwrap();

        assert_eq!(r.cwd, new_dir_s);
        // Verified invariant: as long as the transcript is reachable under "the new cwd's encoded key", claude can find it
        let key = super::encode_project_path(&new_dir_s);
        let landed = projects.join(&key).join(format!("{SID}.jsonl"));
        assert!(landed.exists(), "新键下必须能看到 transcript");
        assert_eq!(
            std::fs::read_to_string(&landed).unwrap(),
            "{\"cwd\":\"x\",\"type\":\"user\"}\n",
            "读到的必须是同一份原文件，不是副本"
        );
        assert!(
            std::fs::symlink_metadata(&landed).unwrap().file_type().is_symlink(),
            "必须是 symlink —— 拷贝会产出同一个 session id 的两份分叉"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Two refusal paths: better to report an error than to leave a dangling symlink or pretend to succeed.
    #[test]
    fn relink_refuses_bad_input() {
        let (root, original_cwd) = sandbox("refuse");
        let projects = root.join("projects");
        let storage = super::encode_project_path(&original_cwd);

        let gone = root.join("not-created").to_string_lossy().into_owned();
        assert!(
            super::recover_session_cwd_in(&projects, "relink", SID, &original_cwd, &gone, &storage).is_err(),
            "目标目录不存在时必须拒绝"
        );
        assert!(
            super::recover_session_cwd_in(&projects, "relink", "no-such-session", &original_cwd, &root.to_string_lossy(), &storage).is_err(),
            "transcript 不存在时必须拒绝，不能建悬空 symlink"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A codex session is **not indexed by cwd** (`~/.codex/sessions/year/month/day/`, `storage_folder` always an empty string),
    /// so it simply has no "key lost" problem -- `codex resume <id>` finds the session from any directory.
    /// This tests: for such a session, "point to new location" only switches the working directory and **must not** touch anything under projects/.
    ///
    /// Without this branch, an empty `storage_folder` makes `projects_root.join("")` land on the projects root itself,
    /// the source file is never found -> it always reports "session record not found" -> the user is stuck in a dialog over a problem that does not exist.
    #[test]
    fn relink_is_a_noop_for_tools_that_dont_key_by_cwd() {
        let (root, original_cwd) = sandbox("codex");
        let projects = root.join("projects");
        let new_dir = root.join("anywhere");
        std::fs::create_dir_all(&new_dir).unwrap();
        let new_dir_s = new_dir.to_string_lossy().into_owned();
        let before: Vec<_> = std::fs::read_dir(&projects).unwrap().flatten().map(|e| e.file_name()).collect();

        let r = super::recover_session_cwd_in(&projects, "relink", SID, &original_cwd, &new_dir_s, "")
            .expect("storage_folder 为空不是错误，是「这个工具不需要修键」");

        assert_eq!(r.cwd, new_dir_s, "就用用户给的新目录");
        let after: Vec<_> = std::fs::read_dir(&projects).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(before, after, "不按 cwd 索引的工具，一个 symlink 都不该建");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// "Rebuild the original directory" creates only the last level; a nonexistent parent directory must be refused.
    ///
    /// `create_dir_all` would create all the missing parents, and an entire missing parent tree usually means an external drive is not mounted.
    /// After creating a real directory at the mount point, that drive would be silently mounted by macOS as "X 1" -- all the user's paths point wrong,
    /// with no warning at all. Better to report an error and let the person mount the drive.
    #[test]
    fn recreate_refuses_when_the_whole_parent_tree_is_gone() {
        let root = std::env::temp_dir().join(format!("makit-recover-parent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let projects = root.join("projects");
        std::fs::create_dir_all(&projects).unwrap();

        // the parent directory missing-volume does not exist either -> this is not "one project directory was deleted"
        let deep = root.join("missing-volume").join("proj");
        let deep_s = deep.to_string_lossy().into_owned();
        let err = super::recover_session_cwd_in(&projects, "recreate", SID, &deep_s, "", "")
            .expect_err("父目录不存在时必须拒绝");
        assert!(err.contains("父目录也不存在"), "报错要说清为什么拒绝，实际: {err}");
        assert!(!deep.exists(), "拒绝了就不许留下任何半成品目录");
        assert!(
            !root.join("missing-volume").exists(),
            "尤其不许在挂载点位置造出真目录"
        );

        // control: only the last level is missing -> rebuild normally
        let shallow = root.join("normal-proj").to_string_lossy().into_owned();
        super::recover_session_cwd_in(&projects, "recreate", SID, &shallow, "", "")
            .expect("只缺最后一级是最常见的场合，必须能重建");
        assert!(std::path::Path::new(&shallow).is_dir());

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Regression: `list_sessions` must skip symlinks when enumerating jsonl.
    ///
    /// After "point to new location" was added, the same transcript is visible under two keys (the original storage directory + the new key).
    /// If enumeration looks only at the extension, the same session_id is scanned as **two entries**, and a pair of twin sessions shows up in the sidebar.
    /// Directory-level symlinks were already skipped (`file_type()` does not follow symlinks, see the comment in the enumeration), while the file-level
    /// case was not handled before -- because nobody had created file-level symlinks at scale.
    #[cfg(unix)]
    #[test]
    fn enumeration_skips_symlinked_jsonl() {
        let dir = std::env::temp_dir().join(format!("makit-enum-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let real = dir.join("real.jsonl");
        std::fs::write(&real, "{}\n").unwrap();
        std::os::unix::fs::symlink(&real, dir.join("linked.jsonl")).unwrap();

        let picked: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| crate::sessions::is_scannable_jsonl(e))
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();

        assert_eq!(picked, vec!["real.jsonl".to_string()], "symlink 的 jsonl 必须被跳过");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod target_missing_tests {
    use super::*;

    #[test]
    fn target_missing_error_is_recognised_in_both_languages() {
        assert!(is_target_missing_error("目标目录不存在: /x/y"));
        assert!(is_target_missing_error("Target folder does not exist: /x/y"));
        assert!(!is_target_missing_error("创建目录失败: permission denied"));
        assert!(!is_target_missing_error(""));
    }
}
