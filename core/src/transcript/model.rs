//! 统一的对话模型（#231）：claude 的 jsonl 和 codex 的 rollout 都解析成这个。
//!
//! 扁平的 `Item` 列表（不是 Turn / Block 嵌套）：方便增量追加，也方便虚拟列表给每一项单独算高度。
//! 连续的 Assistant / ToolCall 在**渲染时**按相邻关系分组，不放进模型。

/// 工具输出保留多少字节：几 MB 的输出不进内存，`total_len` 记原长
pub const MAX_TOOL_TEXT: usize = 16 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    /// 稳定 id：claude 是记录 uuid（一条记录里有多个 Item 时后面的带 `#n`），codex 是行序号
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
    /// 对话被压缩过（compact / microcompact）
    Compacted,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ItemKind {
    /// 真用户输入
    User(String),
    /// 用户贴的图：只留元信息
    Image { media_type: String, bytes: usize },
    /// 斜杠命令回显
    Command { name: String, args: String },
    /// 斜杠命令 / `!` 命令的输出（暗色）
    LocalOutput(String),
    /// `!` 命令的输入
    BashInput(String),
    /// 助手回复，markdown 原文
    Assistant(String),
    /// 思考文本；可能为空（codex 的 reasoning 是加密的）
    Thinking(String),
    /// 工具调用；结果比调用晚到，按 id 配对后挂在 `result` 上
    ToolCall { call_id: String, name: String, input: serde_json::Value, result: Option<ToolResult> },
    /// 提示：recap、后台任务完成、API 重试、被中断……
    Notice { level: Level, text: String },
    Divider(DividerKind),
    /// 一轮花了多久（毫秒），TUI 里的「Baked for 2m 56s」
    TurnDuration(u64),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolResult {
    /// 截断到 `MAX_TOOL_TEXT`（在字符边界上）
    pub text: String,
    pub truncated: bool,
    /// 原始字节数
    pub total_len: usize,
    pub is_error: bool,
    /// 结果里带的图片数
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

/// 读哪种会话文件
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Claude,
    Codex,
}
