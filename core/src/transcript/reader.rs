//! 增量读取器：游标（inode + 已消费字节），只吃完整行。
//!
//! 变化用「可见列表」里的下标报：
//! - `appended`：末尾新增的范围
//! - `updated`：前面已有的 ToolCall 拿到了结果
//! - `reset`：整个重来（文件被截断 / 换了 inode / 当前分支变了，比如回退）——视图应整体重建

use std::fs::File;
use std::io::{self, BufRead, BufReader, Seek, SeekFrom};
use std::ops::Range;
use std::path::PathBuf;

use super::model::{Item, Tool};
use super::state::State;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Changes {
    pub appended: Range<usize>,
    pub updated: Vec<usize>,
    pub reset: bool,
}

pub struct TranscriptReader {
    path: PathBuf,
    tool: Tool,
    state: State,
    inode: Option<u64>,
    /// 已消费到哪（一定停在某个 `\n` 之后）
    offset: u64,
}

impl TranscriptReader {
    pub fn new(path: PathBuf, tool: Tool) -> Self {
        Self { path, tool, state: State::new(tool), inode: None, offset: 0 }
    }

    pub fn poll(&mut self) -> io::Result<Changes> {
        let meta = match std::fs::metadata(&self.path) {
            Ok(m) => m,
            // 文件还没出现（会话刚起、还没写第一条）不是错误
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let n = self.state.visible_len();
                return Ok(Changes { appended: n..n, ..Default::default() });
            }
            Err(e) => return Err(e),
        };
        let inode = inode_of(&meta);
        let mut reset = false;
        if (self.inode.is_some() && inode != self.inode) || meta.len() < self.offset {
            self.state = State::new(self.tool);
            self.offset = 0;
            reset = true;
        }
        self.inode = inode;

        let old: Vec<usize> = (0..self.state.visible_len()).map(|i| self.state.visible_index(i)).collect();
        let mut updated_all: Vec<usize> = Vec::new();

        let mut f = File::open(&self.path)?;
        f.seek(SeekFrom::Start(self.offset))?;
        let mut r = BufReader::with_capacity(256 * 1024, f);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            let n = r.read_until(b'\n', &mut buf)?;
            if n == 0 || buf.last() != Some(&b'\n') {
                break; // 到头了，或者最后一行还没写完换行：留到下次
            }
            self.offset += n as u64;
            let line = String::from_utf8_lossy(&buf);
            updated_all.extend(self.state.feed_line(&line));
        }
        self.state.recompute_visible();

        let new_len = self.state.visible_len();
        let prefix = old.iter().enumerate().take_while(|(i, ix)| *i < new_len && self.state.visible_index(*i) == **ix).count();
        if reset || prefix < old.len() {
            return Ok(Changes { appended: 0..new_len, updated: vec![], reset: true });
        }
        let mut updated: Vec<usize> = updated_all.into_iter().filter_map(|ix| self.state.visible_pos(ix)).filter(|p| *p < old.len()).collect();
        updated.sort_unstable();
        updated.dedup();
        Ok(Changes { appended: old.len()..new_len, updated, reset: false })
    }

    pub fn len(&self) -> usize {
        self.state.visible_len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn item(&self, i: usize) -> &Item {
        self.state.visible(i)
    }

    pub fn invalid_lines(&self) -> usize {
        self.state.invalid_lines()
    }

    pub fn unknown_kinds(&self) -> &std::collections::BTreeMap<String, usize> {
        self.state.unknown_kinds()
    }
}

#[cfg(unix)]
fn inode_of(m: &std::fs::Metadata) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    Some(m.ino())
}

#[cfg(not(unix))]
fn inode_of(_: &std::fs::Metadata) -> Option<u64> {
    None
}
