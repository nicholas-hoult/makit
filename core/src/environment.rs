//! 本机有没有装 Claude Code / Codex（#197）：首次打开没有会话时，要告诉用户是「还没装」还是「装了但还没用过」。
//!
//! 不开 shell 去 `which`：从 Finder 启动的 app 拿到的 PATH 很短（`/usr/bin:/bin:…`），而用户的 claude / codex 多半装在
//! shell 配置里才加进 PATH 的目录（nvm、~/.local/bin……）。所以看两样东西：①命令文件在不在（当前 PATH + 一批常见安装目录）
//! ②会话目录在不在（`~/.claude/projects`、`~/.codex/sessions`，这正是 makit 读的地方）。
//! 检测不到只能说「没检测到」，不能说「没装」：可能装在我们没列到的地方。

use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ToolPresence {
    pub claude_bin: bool,
    pub codex_bin: bool,
    pub claude_data: bool,
    pub codex_data: bool,
}

impl ToolPresence {
    /// Claude Code 的任何一个迹象
    pub fn claude(&self) -> bool {
        self.claude_bin || self.claude_data
    }

    pub fn codex(&self) -> bool {
        self.codex_bin || self.codex_data
    }

    pub fn any(&self) -> bool {
        self.claude() || self.codex()
    }
}

/// 系统级的常见安装目录（Homebrew）。只在真实入口 `detect()` 里并进 PATH，不放进 `detect_in`：
/// 否则测试会读到开发机上真装的 claude / codex，「什么都没装」的用例就不成立
const SYSTEM_BIN_DIRS: &str = "/opt/homebrew/bin:/usr/local/bin";

/// 家目录下常见的命令安装目录（App 的 PATH 里通常没有它们）。nvm 的各个 node 版本目录另外在 `nvm_bin_dirs` 里展开
pub fn common_bin_dirs(home: &Path) -> Vec<PathBuf> {
    let mut v = vec![
        home.join(".local/bin"),
        home.join(".claude/local"),
        home.join(".npm-global/bin"),
        home.join(".bun/bin"),
        home.join(".volta/bin"),
        home.join(".cargo/bin"),
    ];
    v.extend(nvm_bin_dirs(home));
    v
}

/// `~/.nvm/versions/node/<版本>/bin`：nvm 每个 node 版本一个目录，全局装的命令在各自的 bin 里
fn nvm_bin_dirs(home: &Path) -> Vec<PathBuf> {
    let root = home.join(".nvm/versions/node");
    std::fs::read_dir(root).into_iter().flatten().flatten().map(|e| e.path().join("bin")).collect()
}

fn has_command(name: &str, dirs: &[PathBuf]) -> bool {
    dirs.iter().any(|d| d.join(name).is_file())
}

/// 在 `home` 下检测；`path_env` 是 `$PATH` 的值（冒号分隔）
pub fn detect_in(home: &Path, path_env: &str) -> ToolPresence {
    let mut dirs: Vec<PathBuf> = path_env.split(':').filter(|s| !s.is_empty()).map(PathBuf::from).collect();
    dirs.extend(common_bin_dirs(home));
    ToolPresence {
        claude_bin: has_command("claude", &dirs),
        codex_bin: has_command("codex", &dirs),
        claude_data: home.join(".claude/projects").is_dir(),
        codex_data: home.join(".codex/sessions").is_dir(),
    }
}

/// 调试 / 开发用：`MAKIT_TOOLS=none|claude|codex|both` 直接指定检测结果，不看真实的 PATH 和目录。
/// 为什么要有：dev 的假 HOME 只隔离了会话目录，命令检测仍读开发机真实的 PATH，永远看不到「没装」那种提示。
/// 认不出的值返回 None（按真实检测走），不报错
pub fn parse_override(value: &str) -> Option<ToolPresence> {
    let on = |claude, codex| Some(ToolPresence { claude_bin: claude, codex_bin: codex, claude_data: false, codex_data: false });
    match value.trim().to_ascii_lowercase().as_str() {
        "none" => on(false, false),
        "claude" => on(true, false),
        "codex" => on(false, true),
        "both" => on(true, true),
        _ => None,
    }
}

/// 用真实的 HOME 和 PATH 检测（设了 `MAKIT_TOOLS` 就用它指定的结果）
pub fn detect() -> ToolPresence {
    if let Some(p) = std::env::var("MAKIT_TOOLS").ok().and_then(|v| parse_override(&v)) {
        return p;
    }
    match dirs::home_dir() {
        Some(h) => detect_in(&h, &format!("{}:{SYSTEM_BIN_DIRS}", std::env::var("PATH").unwrap_or_default())),
        None => ToolPresence::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// 每个测试自己的临时 HOME（pid + 纳秒 + 名字，不引 tempfile）
    fn temp_home(name: &str) -> PathBuf {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let p = std::env::temp_dir().join(format!("makit-env-{}-{n}-{name}", std::process::id()));
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn touch(p: &Path) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, "").unwrap();
    }

    /// 为什么要测（#197）：错了在界面上就是「明明装了 Claude Code，首次打开却说没找到」（吓跑用户），
    /// 或「没装却说已检测到」（让人以为 makit 坏了）
    #[test]
    fn nothing_installed_is_all_false() {
        let h = temp_home("none");
        assert_eq!(detect_in(&h, ""), ToolPresence::default());
        assert!(!detect_in(&h, "").any());
    }

    #[test]
    fn command_in_path_is_found() {
        let h = temp_home("path");
        let bin = h.join("somewhere/bin");
        touch(&bin.join("claude"));
        let p = detect_in(&h, &format!("/usr/bin:{}", bin.display()));
        assert!(p.claude_bin && !p.codex_bin, "{p:?}");
    }

    #[test]
    fn command_in_common_dir_outside_path_is_found() {
        // Finder 启动的 app PATH 很短：命令装在 ~/.local/bin 也要认出来
        let h = temp_home("common");
        touch(&h.join(".local/bin/codex"));
        let p = detect_in(&h, "/usr/bin:/bin");
        assert!(p.codex_bin && !p.claude_bin, "{p:?}");
    }

    #[test]
    fn nvm_node_version_bins_are_scanned() {
        let h = temp_home("nvm");
        touch(&h.join(".nvm/versions/node/v22.1.0/bin/claude"));
        assert!(detect_in(&h, "").claude_bin);
    }

    #[test]
    fn session_directories_count_as_traces() {
        let h = temp_home("data");
        fs::create_dir_all(h.join(".claude/projects")).unwrap();
        let p = detect_in(&h, "");
        assert!(p.claude_data && !p.codex_data && p.claude() && !p.codex() && p.any(), "{p:?}");
        fs::create_dir_all(h.join(".codex/sessions")).unwrap();
        assert!(detect_in(&h, "").codex_data);
    }

    #[test]
    fn a_directory_named_like_the_command_is_not_the_command() {
        let h = temp_home("dir");
        fs::create_dir_all(h.join(".local/bin/claude")).unwrap(); // 目录，不是可执行文件
        assert!(!detect_in(&h, "").claude_bin);
    }

    /// 为什么要测：覆盖开关写错，dev 里想看的提示就出不来，又得怀疑是不是提示本身坏了
    #[test]
    fn override_presets() {
        let on = |c, x| ToolPresence { claude_bin: c, codex_bin: x, claude_data: false, codex_data: false };
        assert_eq!(parse_override("none"), Some(on(false, false)));
        assert_eq!(parse_override("claude"), Some(on(true, false)));
        assert_eq!(parse_override("codex"), Some(on(false, true)));
        assert_eq!(parse_override("both"), Some(on(true, true)));
        assert_eq!(parse_override(" BOTH "), Some(on(true, true)), "忽略大小写和空白");
        assert_eq!(parse_override("乱写"), None, "认不出就按真实检测走");
        assert_eq!(parse_override(""), None);
    }
}
