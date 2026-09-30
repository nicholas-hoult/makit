//! 读会话文件的一次增量轮询（查看对话面板、可重排视图共用）。
//!
//! `poll` 在后台线程里跑：reader 读新增的完整行，变化的 Item 克隆出来交给前台，前台不碰 reader。

use makit_core::transcript::{Changes, Item, TranscriptReader};

/// 后台线程一次 `poll()` 的结果：Item 已经克隆出来，前台不碰 reader
pub struct Poll {
    pub changes: Changes,
    pub total: usize,
    pub appended: Vec<Item>,
    pub updated: Vec<(usize, Item)>,
}

pub fn poll(reader: &mut TranscriptReader) -> std::io::Result<Poll> {
    let changes = reader.poll()?;
    let appended = changes.appended.clone().map(|i| reader.item(i).clone()).collect();
    let updated = changes.updated.iter().map(|&i| (i, reader.item(i).clone())).collect();
    Ok(Poll { total: reader.len(), changes, appended, updated })
}
