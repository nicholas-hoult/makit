//! makit-core：makit 的业务逻辑，不依赖任何 UI 框架（#226）。
//!
//! Tauri 版（`src-tauri/`）只剩命令薄转发 + emit；GPUI 版（`native/`）直接调这里。
//!
//! | 模块 | 内容 |
//! |---|---|
//! | `sessions` | 会话扫描（`list_sessions` 全量 / `list_sessions_by_paths` 增量）、详情、单个 meta、按 cwd 反查 |
//! | `scan_cache` | jsonl 增量游标（inode + offset），落盘 `scan-cache.json` |
//! | `running` | 运行状态（`~/.claude/sessions/*.json`）、pty_id → session 绑定 |
//! | `process` | 进程表、进程环境变量、后代进程、关 tab 杀进程（进程树 + `MAKIT_PTY_ID` 标记） |
//! | `recovery` | 启动目录丢失后的恢复（重建 / 指到新位置）、存储键 symlink |
//! | `archive` | 归档（`archived.json`） |
//! | `paths` | 打开路径、批量判存在、工具 logo |
//! | `watcher` | 会话目录监听，回调 `WatchEvent` |
//! | `hook` | Claude Code Notification hook：脚本安装 + unix socket 服务，回调每一行 |
//! | `pty_prep` | PTY 启动前的纯逻辑：cwd 校正 / cwd 闸 / resume id / shell 集成文件 |
//! | `pty_batch` | PTY 输出合并节奏（Tauri 版 emit 用） |
//! | `perf` | perf.log 埋点 |
//! | `ai_provider` | Codex 会话解析 |
//! | `transcript` | 对话模型：claude jsonl / codex rollout → 统一 `Item` 列表 + 增量读取器（#231） |
//! | `worktree` | git worktree 列表 |

pub mod ai_provider;
pub mod archive;
pub mod hook;
pub mod paths;
pub mod perf;
pub mod process;
pub mod pty_batch;
pub mod pty_prep;
pub mod recovery;
pub mod running;
pub mod scan_cache;
pub mod sessions;
pub mod transcript;
pub mod watcher;
pub mod worktree;

pub use process::ProcessInfo;
pub use running::{PtyBinding, RunningMeta};
pub use sessions::{ConversationMessage, SessionMeta};
