//! 会话启动目录被删 / 搬走之后的恢复（#173）：存储键编码、symlink、重建或指到新位置。

use serde::Serialize;
use std::fs;
use std::path::Path;

use crate::paths::projects_dir;

/// claude 的存储键：把 cwd 逐字符编码成 `~/.claude/projects/` 下的目录名。
///
/// **先 canonicalize 再编码**。claude 是拿自己进程的 cwd 算这个键的，而进程 cwd 永远是
/// 解析过软链的实路径（macOS 上 `/var` = `/private/var`、`/tmp` = `/private/tmp`）。
/// 不解析就会算出一个 claude 永远不会用的键 —— 建 symlink 会建到错的地方，「恢复会话」
/// 静默失效（实测踩过，见 `session_recovery_tests`）。
///
/// 路径不存在时 canonicalize 失败，退回原样编码。这正是「原目录已被删」的场合，而那种
/// 场合退回原样是对的：jsonl 里记的 cwd 本身就是 claude 当年写下的实路径。
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
    let dir = projects_dir().ok_or("无法定位 home 目录")?;
    let encoded = encode_project_path(&cwd);
    if encoded == storage_folder {
        return Ok("无需处理".into());
    }
    let session_file = format!("{}.jsonl", session_id);
    let target_dir = dir.join(&encoded);
    let target_file = target_dir.join(&session_file);
    if target_file.exists() {
        return Ok("已存在".into());
    }
    let source_file = dir.join(&storage_folder).join(&session_file);
    if !source_file.exists() {
        return Err(format!("源文件不存在: {}/{}", storage_folder, session_file));
    }
    #[cfg(unix)]
    {
        // 目标目录不存在 → symlink 整个目录；已存在 → symlink 单个文件
        if !target_dir.exists() {
            let source_dir = dir.join(&storage_folder);
            std::os::unix::fs::symlink(&source_dir, &target_dir)
                .map_err(|e| format!("创建目录 symlink 失败: {}", e))?;
        } else {
            std::os::unix::fs::symlink(&source_file, &target_file)
                .map_err(|e| format!("创建文件 symlink 失败: {}", e))?;
        }
    }
    Ok(format!("symlink: {}/{}", encoded, session_file))
}

#[derive(Serialize, Debug)]
pub struct RecoveredSession {
    /// 恢复之后该用哪个 cwd 去 resume
    pub cwd: String,
    /// 到底动了什么（要能对用户交代清楚，不能只说"成功"）
    pub detail: String,
}

/// 启动目录被删之后恢复会话。`projects_root` 可注入，测试才不会写进真的 `~/.claude`。
///
/// 本质：会话数据一个字节都没丢，丢的只是**一把钥匙**。`claude -r <id>` 只会去
/// `projects/<encode(realpath(cwd))>/<id>.jsonl` 找，目录被删就意味着你再也算不出
/// 那个键。所以两个动作都只做一件事 —— **让钥匙对上**：
///
/// - `recreate`：把原目录建回来（空的）。键本来就是按原路径算的，建回来就自然对上，
///   `projects/` 一个字节都不用碰。代码没了，但会话能接着聊。
/// - `relink`：代码搬家了，指到新目录。在新键下放一个指向原 transcript 的
///   **文件级** symlink。不用目录级：那会把老项目下所有会话一并暴露到新键下。
///
/// 两种模式都**绝不拷贝** jsonl —— 拷贝会产出同一个 session id 的两份分叉。
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
            // 只补**最后一级**，父目录必须已经存在。用 `create_dir_all` 会把缺失的父目录
            // 全部造出来，而「一整棵父树都没了」通常不是「删了一个项目目录」，是外置盘
            // 没挂载 / 整个上级被搬走。那种情况下在挂载点上造一个真目录后果很实：
            // 那块盘之后会被 macOS 挂成「X 1」，而且这事没有任何提示。
            let parent = Path::new(original_cwd).parent();
            match parent {
                Some(p) if !p.as_os_str().is_empty() && !p.is_dir() => {
                    return Err(format!(
                        "父目录也不存在：{}。这看起来不只是删掉了一个项目目录（可能是外置盘没挂载，或整个上级被搬走了），\
                         没有替你把整棵目录树造出来。挂上盘、或改用「指到新位置」",
                        p.display()
                    ));
                }
                _ => {}
            }
            fs::create_dir_all(original_cwd).map_err(|e| format!("重建目录失败: {}", e))?;
            Ok(RecoveredSession {
                cwd: original_cwd.to_string(),
                detail: format!("已重建空目录 {}（未改动 ~/.claude）", original_cwd),
            })
        }
        "relink" => {
            if !Path::new(target_cwd).is_dir() {
                return Err(format!("目标目录不存在: {}", target_cwd));
            }
            // codex 的会话按**日期**分层存在 `~/.codex/sessions/年/月/日/` 下，**不是 cwd 键**
            // （`ai_provider.rs` 里 codex 的 `storage_folder` 恒为空串就是这个意思）。
            // 也就是说 codex 根本没有这把钥匙可丢 —— `codex resume <id>` 在任何目录都能找到会话。
            // 所以这里没有键要修，「指到新位置」就只是换个工作目录，一个 symlink 都不该建。
            if storage_folder.is_empty() {
                return Ok(RecoveredSession {
                    cwd: target_cwd.to_string(),
                    detail: format!("已切到 {}（这个工具的会话不按目录索引，无需改动存储）", target_cwd),
                });
            }
            let session_file = format!("{}.jsonl", session_id);
            let source = projects_root.join(storage_folder).join(&session_file);
            // 先确认源在，再动手 —— 否则会留下一个悬空 symlink，比报错更难查
            if !source.exists() {
                return Err(format!("找不到会话记录: {}/{}", storage_folder, session_file));
            }
            let key = encode_project_path(target_cwd);
            if key == storage_folder {
                return Ok(RecoveredSession {
                    cwd: target_cwd.to_string(),
                    detail: "钥匙本来就对得上，无需处理".into(),
                });
            }
            let target_dir = projects_root.join(&key);
            // 真目录 + 里面放文件级 symlink，不建目录级 symlink
            fs::create_dir_all(&target_dir).map_err(|e| format!("创建目录失败: {}", e))?;
            let target_file = target_dir.join(&session_file);
            if !target_file.exists() {
                #[cfg(unix)]
                std::os::unix::fs::symlink(&source, &target_file)
                    .map_err(|e| format!("创建 symlink 失败: {}", e))?;
            }
            Ok(RecoveredSession {
                cwd: target_cwd.to_string(),
                detail: format!("已在 {}/ 下建软链指向原会话记录", key),
            })
        }
        other => Err(format!("未知恢复方式: {}", other)),
    }
}

/// 目录还在不在。刻意**不做**成 `SessionMeta.cwd_exists`：`list_sessions` 里每次 stat
/// 本机实测约 0.7ms、300 多个文件合计约 0.5s，已经是现存的性能痛点（#144），
/// 每个会话再加一次 stat 是往已知热路径上加钱。而这个信息只有「用户点开会话的那一刻」
/// 才用得到，那时候一次 stat 就够。
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
    let root = projects_dir().ok_or("无法定位 home 目录")?;
    recover_session_cwd_in(&root, &mode, &session_id, &original_cwd, &target_cwd, &storage_folder)
}

/// #173「会话启动目录被删了怎么恢复」。
///
/// 本质：会话数据一个字节都没丢，丢的只是**一把钥匙**。`claude -r <id>` 找会话的唯一
/// 方式是去 `~/.claude/projects/<encode(realpath(cwd))>/<id>.jsonl` 里找 —— 目录被删、
/// 你在别处启动，算出来的键就不一样，claude 报「No conversation found」，而 transcript
/// 还老老实实躺在原存储目录里。所以「恢复会话」= **让钥匙对上**，不是找回数据；任何
/// 拷贝 jsonl 的做法都会产出同一个 session id 的两份分叉，方向就是错的。
///
/// 实测（隔离沙箱，`CLAUDE_CONFIG_DIR` 指向临时目录，没碰真实 `~/.claude`）：
///   - 真 id、键不匹配 → `No conversation found with session ID`
///   - 同一个 id，**全新的空目录** + transcript 放在该目录的编码键下 → 找到了
///     （报错变成「Provide a prompt to continue」）
/// 也就是说**原目录完全不需要存在**，任何一个存在的目录都行。
#[cfg(test)]
mod session_recovery_tests {
    /// 回归：编码存储键前**必须先 canonicalize**。
    ///
    /// claude 是拿自己进程的 cwd 算这个键的，而进程 cwd 永远是解析过软链的实路径 ——
    /// macOS 上 `/var` 就是 `/private/var` 的软链，`/tmp` 是 `/private/tmp`。不 canonicalize
    /// 就会给含软链的路径算出一个 claude 永远不会用的键，symlink 建在错的地方，
    /// 「恢复」静默失效。
    ///
    /// 这个坑是实测踩出来的：第一次做恢复实验时手工造的键用的是 `/var/folders/…`，
    /// claude 算的是 `/private/var/folders/…`，于是明明 transcript 就在那儿也报
    /// 「No conversation found」，白跑一轮才发现不是机制不对、是键算错了。
    #[test]
    fn encode_canonicalizes_before_encoding() {
        let raw = std::env::temp_dir().join(format!("makit-enc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&raw);
        std::fs::create_dir_all(&raw).unwrap();
        let raw_str = raw.to_string_lossy().into_owned();
        let canonical = std::fs::canonicalize(&raw).unwrap().to_string_lossy().into_owned();

        // 前提：本机的临时目录确实经过软链，否则这条测试什么都没测到
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

    /// 路径不存在时 canonicalize 会失败，此时必须退回逐字符编码原样 —— 而这恰恰是
    /// 「原目录已被删」的场合。这种场合下退回原样是**正确**的：jsonl 里记的 cwd 本身
    /// 就是 claude 当年写下的实路径，不需要也无法再解析。
    #[test]
    fn encode_falls_back_to_raw_when_path_is_gone() {
        assert_eq!(
            super::encode_project_path("/definitely/not/a/path/ai-claw.studio"),
            "-definitely-not-a-path-ai-claw-studio"
        );
    }

    const SID: &str = "4110cea1-8771-4e89-b521-b93f5a677c5a";

    /// 造一个假的 `projects/` 根 + 一个装着 transcript 的存储目录。
    /// 绝不碰真实 `~/.claude`：所有恢复逻辑都收口成 `*_in(projects_root, …)`，
    /// 就是为了让测试能指到别处。
    fn sandbox(tag: &str) -> (std::path::PathBuf, String) {
        let root = std::env::temp_dir().join(format!("makit-recover-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let projects = root.join("projects");
        // 原 cwd 刻意放在 root 下面并且**不创建**：模拟「启动目录已被删」
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

    /// 动作 A「重建原目录」：键本来就是按原路径算的，把空目录建回来钥匙立刻对上。
    /// 关键断言是**一个 symlink 都不许建** —— 这是三条恢复路径里唯一完全不碰
    /// claude 数据目录的一条，它的价值就在这儿。
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

    /// 动作 B「指到新位置」：代码搬家了，要在新目录里继续。
    /// 用**文件级** symlink 而不是目录级：目录级会把老项目下所有会话一并暴露到新键下，
    /// 而且 `list_sessions` 的枚举会踩上去。
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
        // 实测过的不变量：transcript 只要在「新 cwd 的编码键」下可达，claude 就能找到
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

    /// 两条拒绝路径：宁可报错，也不留一个悬空 symlink 或者假装成功。
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

    /// codex 的会话**不按 cwd 索引**（`~/.codex/sessions/年/月/日/`，`storage_folder` 恒为空串），
    /// 所以它压根没有「钥匙丢了」这个问题 —— `codex resume <id>` 在任何目录都能找到会话。
    /// 这条测的是：这种会话走「指到新位置」时只换工作目录，**不许**在 projects/ 下动任何东西。
    ///
    /// 不加这个分支的话，空 `storage_folder` 会让 `projects_root.join("")` 落回 projects 根身上，
    /// 源文件永远找不到 → 永远报「找不到会话记录」→ 用户为一个不存在的问题卡在对话框里。
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

    /// 「重建原目录」只补最后一级，父目录不存在必须拒绝。
    ///
    /// `create_dir_all` 会把缺失的父目录全造出来，而整棵父树都没了通常意味着外置盘没挂载。
    /// 在挂载点上造一个真目录之后，那块盘会被 macOS 静默挂成「X 1」——用户的路径全指错，
    /// 而且没有任何提示。宁可报错让人去挂盘。
    #[test]
    fn recreate_refuses_when_the_whole_parent_tree_is_gone() {
        let root = std::env::temp_dir().join(format!("makit-recover-parent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let projects = root.join("projects");
        std::fs::create_dir_all(&projects).unwrap();

        // 父目录 missing-volume 也不存在 → 这不是「删了一个项目目录」
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

        // 对照：只缺最后一级 → 正常重建
        let shallow = root.join("normal-proj").to_string_lossy().into_owned();
        super::recover_session_cwd_in(&projects, "recreate", SID, &shallow, "", "")
            .expect("只缺最后一级是最常见的场合，必须能重建");
        assert!(std::path::Path::new(&shallow).is_dir());

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 回归：`list_sessions` 枚举 jsonl 时必须跳过 symlink。
    ///
    /// 加了「指到新位置」之后，同一份 transcript 会在两个键下可见（原存储目录 + 新键）。
    /// 枚举时只看扩展名的话，同一个 session_id 会被扫成**两条**，侧栏里出现一对孪生会话。
    /// 目录级 symlink 本来就被跳过（`file_type()` 不跟随软链，见枚举那段注释），文件级
    /// 这条以前没管 —— 因为以前没人成规模地建过文件级 symlink。
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
