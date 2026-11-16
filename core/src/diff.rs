//! Uncommitted diff stat for a session's cwd (#310): what has this agent changed so far,
//! relative to HEAD (working tree + staged, merged). Not a full diff viewer — just the
//! file list and added/removed line counts, same grain as `git diff --stat`.

use std::process::Command;

use serde::{Deserialize, Serialize};

/// Cap on how many changed files are kept, so a huge diff doesn't blow up the panel.
const MAX_FILES: usize = 50;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct FileDiff {
    pub path: String,
    pub added: u32,
    pub removed: u32,
    /// `git diff --numstat` reports `-`/`-` for binary files; counts above are 0 in that case.
    pub binary: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct DiffStat {
    pub files: Vec<FileDiff>,
    pub truncated: bool,
}

/// Uncommitted changes (working tree + staged) at `cwd`, relative to HEAD.
/// `None` when `cwd` isn't a git repo (also covers a repo that no longer exists on disk —
/// both are "nothing to say", the caller doesn't need to tell them apart).
pub fn uncommitted_diff(cwd: &str) -> Option<DiffStat> {
    if !is_git_repo(cwd) {
        return None;
    }
    let mut files = run_numstat(cwd, &["diff", "--numstat"]);
    merge_numstat(&mut files, run_numstat(cwd, &["diff", "--cached", "--numstat"]));
    let truncated = files.len() > MAX_FILES;
    files.truncate(MAX_FILES);
    Some(DiffStat { files, truncated })
}

fn is_git_repo(cwd: &str) -> bool {
    Command::new("git")
        .args(["-C", cwd, "rev-parse", "--is-inside-work-tree"])
        .output()
        .map(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == "true")
        .unwrap_or(false)
}

fn run_numstat(cwd: &str, args: &[&str]) -> Vec<FileDiff> {
    let output = match Command::new("git").args(["-C", cwd]).args(args).output() {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };
    parse_numstat(&String::from_utf8_lossy(&output.stdout))
}

fn parse_numstat(text: &str) -> Vec<FileDiff> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, '\t');
            let added = parts.next()?;
            let removed = parts.next()?;
            let path = parts.next()?;
            let binary = added == "-" || removed == "-";
            Some(FileDiff {
                path: path.to_string(),
                added: if binary { 0 } else { added.parse().unwrap_or(0) },
                removed: if binary { 0 } else { removed.parse().unwrap_or(0) },
                binary,
            })
        })
        .collect()
}

/// Merge `--cached` results into the working-tree list: a file touched on both sides
/// (staged some lines, then kept editing) gets its counts summed — an overview total,
/// same simplification as `git status --short` showing it as one line either way.
fn merge_numstat(acc: &mut Vec<FileDiff>, extra: Vec<FileDiff>) {
    for f in extra {
        match acc.iter_mut().find(|e| e.path == f.path) {
            Some(existing) => {
                existing.added += f.added;
                existing.removed += f.removed;
                existing.binary = existing.binary || f.binary;
            }
            None => acc.push(f),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};

    /// Each test's own scratch git repo (pid + nanoseconds, no new crate — same style as `environment.rs::temp_home`).
    fn temp_repo(name: &str) -> PathBuf {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let p = std::env::temp_dir().join(format!("makit-diff-{}-{n}-{name}", std::process::id()));
        fs::create_dir_all(&p).unwrap();
        run(&p, &["init", "-q"]);
        run(&p, &["config", "user.email", "test@example.com"]);
        run(&p, &["config", "user.name", "test"]);
        p
    }

    fn run(dir: &Path, args: &[&str]) {
        let status = Command::new("git").args(["-C"]).arg(dir).args(args).status().unwrap();
        assert!(status.success(), "git {args:?} failed in {dir:?}");
    }

    fn write(dir: &Path, rel: &str, content: &str) {
        let p = dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }

    fn commit_all(dir: &Path, msg: &str) {
        run(dir, &["add", "-A"]);
        run(dir, &["commit", "-q", "-m", msg]);
    }

    fn path<'a>(stat: &'a DiffStat, want: &str) -> &'a FileDiff {
        stat.files.iter().find(|f| f.path == want).unwrap_or_else(|| panic!("{want} not in {stat:?}"))
    }

    #[test]
    fn not_a_git_repo_is_none() {
        let p = std::env::temp_dir().join(format!("makit-diff-not-git-{}", std::process::id()));
        fs::create_dir_all(&p).unwrap();
        assert_eq!(uncommitted_diff(p.to_str().unwrap()), None);
    }

    #[test]
    fn a_repo_path_that_no_longer_exists_is_none() {
        let p = temp_repo("gone");
        fs::remove_dir_all(&p).unwrap();
        assert_eq!(uncommitted_diff(p.to_str().unwrap()), None);
    }

    #[test]
    fn clean_repo_is_some_empty() {
        let p = temp_repo("clean");
        write(&p, "a.txt", "hello\n");
        commit_all(&p, "init");
        assert_eq!(uncommitted_diff(p.to_str().unwrap()), Some(DiffStat { files: vec![], truncated: false }));
    }

    #[test]
    fn working_tree_edit_is_counted() {
        let p = temp_repo("wt");
        write(&p, "a.txt", "one\ntwo\nthree\n");
        commit_all(&p, "init");
        write(&p, "a.txt", "one\ntwo\nTHREE\nfour\n");
        let stat = uncommitted_diff(p.to_str().unwrap()).unwrap();
        let f = path(&stat, "a.txt");
        assert_eq!((f.added, f.removed, f.binary), (2, 1, false));
    }

    #[test]
    fn staged_edit_is_counted() {
        let p = temp_repo("staged");
        write(&p, "a.txt", "one\n");
        commit_all(&p, "init");
        write(&p, "a.txt", "one\ntwo\n");
        run(&p, &["add", "a.txt"]);
        let stat = uncommitted_diff(p.to_str().unwrap()).unwrap();
        let f = path(&stat, "a.txt");
        assert_eq!((f.added, f.removed), (1, 0));
    }

    #[test]
    fn staged_then_further_edited_sums_both_sides() {
        let p = temp_repo("both");
        write(&p, "a.txt", "one\n");
        commit_all(&p, "init");
        write(&p, "a.txt", "one\ntwo\n");
        run(&p, &["add", "a.txt"]); // staged: +1
        write(&p, "a.txt", "one\ntwo\nthree\n"); // working tree on top: +1 more
        let stat = uncommitted_diff(p.to_str().unwrap()).unwrap();
        let f = path(&stat, "a.txt");
        assert_eq!(f.added, 2, "staged +1 and working-tree +1 on the same file sum to +2");
    }

    #[test]
    fn a_brand_new_untracked_file_is_not_in_plain_diff() {
        // Documents git's own boundary: `git diff` doesn't show untracked files.
        // Not a bug to "fix" here — this test pins the behavior so it isn't mistaken for one later.
        let p = temp_repo("untracked");
        write(&p, "a.txt", "one\n");
        commit_all(&p, "init");
        write(&p, "new.txt", "brand new\n");
        let stat = uncommitted_diff(p.to_str().unwrap()).unwrap();
        assert!(stat.files.iter().all(|f| f.path != "new.txt"));
    }

    #[test]
    fn a_binary_file_is_flagged_not_counted() {
        let p = temp_repo("binary");
        fs::write(p.join("a.bin"), [0u8, 1, 2, 255]).unwrap();
        commit_all(&p, "init");
        fs::write(p.join("a.bin"), [0u8, 1, 2, 3, 4]).unwrap();
        let stat = uncommitted_diff(p.to_str().unwrap()).unwrap();
        let f = path(&stat, "a.bin");
        assert_eq!((f.added, f.removed, f.binary), (0, 0, true));
    }

    #[test]
    fn more_than_max_files_is_truncated() {
        let p = temp_repo("many");
        for i in 0..60 {
            write(&p, &format!("f{i}.txt"), "one\n");
        }
        commit_all(&p, "init");
        for i in 0..60 {
            write(&p, &format!("f{i}.txt"), "one\ntwo\n");
        }
        let stat = uncommitted_diff(p.to_str().unwrap()).unwrap();
        assert_eq!(stat.files.len(), MAX_FILES);
        assert!(stat.truncated);
    }
}
