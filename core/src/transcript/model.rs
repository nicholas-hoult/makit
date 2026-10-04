//! Unified conversation model (#231): both claude's jsonl and codex's rollout are parsed into this.
//!
//! A flat `Item` list (not nested Turn / Block): easy to append incrementally, and lets a virtual list compute each item's height independently.
//! Consecutive Assistant / ToolCall items are grouped **at render time** by adjacency, not in the model.

/// How many bytes of tool output to keep: output of several MB stays out of memory; `total_len` records the original length
pub const MAX_TOOL_TEXT: usize = 16 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    /// Stable id: for claude it is the record uuid (when one record yields several Items, later ones get `#n`); for codex it is the line number
    pub id: String,
    pub time: Option<String>,
    pub kind: ItemKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Info,
    Warn,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DividerKind {
    /// The conversation was compacted (compact / microcompact)
    Compacted,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ItemKind {
    /// Genuine user input
    User(String),
    /// An image pasted by the user: only metadata is kept
    Image { media_type: String, bytes: usize },
    /// Echo of a slash command
    Command { name: String, args: String },
    /// Output of a slash command / `!` command (dim)
    LocalOutput(String),
    /// Input of a `!` command
    BashInput(String),
    /// Assistant reply, raw markdown
    Assistant(String),
    /// Thinking text; may be empty (codex's reasoning is encrypted)
    Thinking(String),
    /// Tool call; the result arrives later than the call and is attached to `result` by id pairing
    ToolCall { call_id: String, name: String, input: serde_json::Value, result: Option<ToolResult> },
    /// Notices: recap, background task finished, API retry, interruption...
    Notice { level: Level, text: String },
    Divider(DividerKind),
    /// How long a turn took (milliseconds), the "Baked for 2m 56s" in the TUI
    TurnDuration(u64),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolResult {
    /// Truncated to `MAX_TOOL_TEXT` (on a character boundary)
    pub text: String,
    pub truncated: bool,
    /// Original byte count
    pub total_len: usize,
    pub is_error: bool,
    /// Number of images in the result
    pub images: usize,
}

impl ToolResult {
    pub fn new(full: &str, is_error: bool, images: usize) -> Self {
        let total_len = full.len();
        if total_len <= MAX_TOOL_TEXT {
            return Self { text: full.to_string(), truncated: false, total_len, is_error, images };
        }
        let mut end = MAX_TOOL_TEXT;
        while !full.is_char_boundary(end) {
            end -= 1;
        }
        Self { text: full[..end].to_string(), truncated: true, total_len, is_error, images }
    }
}

/// Which kind of session file to read
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Claude,
    Codex,
}
