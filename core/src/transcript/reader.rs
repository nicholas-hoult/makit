//! Incremental reader: a cursor (inode + consumed bytes) that only consumes complete lines.
//!
//! Changes are reported as indices into the "visible list":
//! - `appended`: the range newly added at the end
//! - `updated`: an earlier ToolCall received its result
//! - `reset`: start over entirely (file truncated / inode changed / current branch changed, e.g. a rewind); the view must be rebuilt

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
    /// How far we have consumed (always right after a `\n`)
    offset: u64,
}

impl TranscriptReader {
    pub fn new(path: PathBuf, tool: Tool) -> Self {
        Self { path, tool, state: State::new(tool), inode: None, offset: 0 }
    }

    pub fn poll(&mut self) -> io::Result<Changes> {
        let meta = match std::fs::metadata(&self.path) {
            Ok(m) => m,
            // A file that does not exist yet (session just started, nothing written) is not an error
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

        let old_len = self.state.visible_len();
        let mut updated_all: Vec<usize> = Vec::new();

        let mut f = File::open(&self.path)?;
        f.seek(SeekFrom::Start(self.offset))?;
        let mut r = BufReader::with_capacity(256 * 1024, f);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            let n = r.read_until(b'\n', &mut buf)?;
            if n == 0 || buf.last() != Some(&b'\n') {
                break; // reached the end, or the last line has no newline yet: leave it for next time
            }
            self.offset += n as u64;
            let line = String::from_utf8_lossy(&buf);
            updated_all.extend(self.state.feed_line(&line));
        }
        let prefix_kept = self.state.recompute_visible();

        let new_len = self.state.visible_len();
        if reset || !prefix_kept {
            return Ok(Changes { appended: 0..new_len, updated: vec![], reset: true });
        }
        let mut updated: Vec<usize> = updated_all.into_iter().filter_map(|ix| self.state.visible_pos(ix)).filter(|p| *p < old_len).collect();
        updated.sort_unstable();
        updated.dedup();
        Ok(Changes { appended: old_len..new_len, updated, reset: false })
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
