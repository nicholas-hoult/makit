//! Conversation model (#231 phase 1): parses claude's jsonl / codex's rollout into a unified `Item` list, with an incremental reader.
//!
//! No UI dependency. Design, measured facts about real files, parsing rules and the test plan are in the private repo, #231 TRD section 11.
//!
//! - `model`: `Item` / `ItemKind` / `ToolResult`
//! - `state`: the parsing state machine fed line by line (never touches files; tests use it)
//! - `claude` / `codex`: per-line parsers for the two formats
//! - `reader`: `TranscriptReader`, file reading + cursor + incremental updates

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
