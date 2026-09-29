//! 对话模型（#231 第 1 期）：把 claude 的 jsonl / codex 的 rollout 解析成统一的 `Item` 列表，带增量读取器。
//!
//! 不依赖任何界面。设计、真实文件的实测事实、解析规则、测试方案见私有仓库 #231 TRD §11。
//!
//! - `model`：`Item` / `ItemKind` / `ToolResult`
//! - `state`：逐行喂入的解析状态机（不碰文件，测试用它）
//! - `claude` / `codex`：两种格式的逐行解析
//! - `reader`：`TranscriptReader`，读文件 + 游标 + 增量

pub mod claude;
pub mod codex;
pub mod model;
pub mod reader;
pub mod state;

#[cfg(test)]
mod tests;

pub use model::{DividerKind, Item, ItemKind, Level, Tool, ToolResult, MAX_TOOL_TEXT};
pub use reader::{Changes, TranscriptReader};
pub use state::State;
