//! makit-core: makit's business logic, independent of any UI framework (#226).
//!
//! The Tauri version (`src-tauri/`) is reduced to thin command forwarding + emit; the GPUI version (`native/`) calls this directly.
//!
//! | Module | Contents |
//! |---|---|
//! | `sessions` | Session scanning (`list_sessions` full / `list_sessions_by_paths` incremental), details, single meta, reverse lookup by cwd |
//! | `scan_cache` | Incremental jsonl cursor (inode + offset), persisted to `scan-cache.json` |
//! | `running` | Running state (`~/.claude/sessions/*.json`), pty_id -> session binding |
//! | `process` | Process table, process environment variables, descendant processes, killing processes when a tab closes (process tree + `MAKIT_PTY_ID` marker) |
//! | `recovery` | Recovery after the launch directory is lost (rebuild / point to a new location), storage-key symlink |
//! | `archive` | Archive (`archived.json`) |
//! | `paths` | Opening paths, batch existence checks, tool logos |
//! | `watcher` | Session directory watching, calls back with `WatchEvent` |
//! | `hook` | Claude Code Notification hook: script installation + unix socket server, calls back for each line |
//! | `pty_prep` | Pure logic before PTY launch: cwd correction / cwd gate / resume id / shell integration files |
//! | `pty_batch` | PTY output coalescing cadence (used by the Tauri version's emit) |
//! | `perf` | perf.log instrumentation |
//! | `ai_provider` | Codex session parsing |
//! | `transcript` | Conversation model: claude jsonl / codex rollout -> unified `Item` list + incremental reader (#231) |
//! | `worktree` | git worktree listing |

pub mod i18n;
pub mod ai_provider;
pub mod archive;
pub mod environment;
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

// UI-facing strings: `locales/{zh,en}.json`, looked up with `ts!("key")`. Chinese is the fallback.
rust_i18n::i18n!("locales", fallback = "zh");
