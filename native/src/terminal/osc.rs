//! OSC 7（shell 报告当前目录）的流式识别（对齐清单不变量 17）。
//!
//! alacritty_terminal 的解析器不处理 OSC 7（直接丢掉），所以在 PTY 读出来的字节流上先过一遍这个扫描器
//! （`pty.rs` 的 `ScanningPty` 包在 alacritty 的 tty 外面），认出 `ESC ] 7 ; file://host/path (BEL | ESC \)`，
//! 解码出路径，原字节照样交给 alacritty 解析。shell 那头由 core 的 `prepare_zsh_integration`（ZDOTDIR）在
//! chpwd 和 precmd 两处发。
//!
//! 为什么单独测：PTY 一次读多少字节是随机的，一条 OSC 7 常被切在两次读之间；错了的表现是
//! 「cd 之后 ⌘+点击相对路径打开的是旧目录下的文件」，很难在界面上发现。

/// 一条 OSC 最长收多少字节（防止坏数据让缓冲无限涨）
const MAX_OSC: usize = 4096;

#[derive(Default)]
pub struct Osc7Scanner {
    state: State,
    buf: Vec<u8>,
}

#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    #[default]
    Ground,
    Esc,
    /// 在 OSC 里，收正文
    Osc,
    /// OSC 正文里遇到 ESC（可能是 ST 的开头）
    OscEsc,
}

impl Osc7Scanner {
    /// 喂一段字节，返回这段里完整出现的 OSC 7 目录（已解码）
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<String> {
        let mut out = Vec::new();
        for &b in bytes {
            self.state = match (self.state, b) {
                (State::Ground, 0x1b) => State::Esc,
                (State::Ground, _) => State::Ground,
                (State::Esc, b']') => {
                    self.buf.clear();
                    State::Osc
                }
                (State::Esc, 0x1b) => State::Esc,
                (State::Esc, _) => State::Ground,
                (State::Osc, 0x07) => {
                    self.finish(&mut out);
                    State::Ground
                }
                (State::Osc, 0x1b) => State::OscEsc,
                (State::Osc, b) => {
                    if self.buf.len() < MAX_OSC {
                        self.buf.push(b);
                    }
                    State::Osc
                }
                // ESC \ = ST，正常结束；ESC 后面跟别的 = OSC 被打断，按新的转义序列开头处理
                (State::OscEsc, b'\\') => {
                    self.finish(&mut out);
                    State::Ground
                }
                (State::OscEsc, b']') => {
                    self.buf.clear();
                    State::Osc
                }
                (State::OscEsc, _) => State::Ground,
            };
        }
        out
    }

    fn finish(&mut self, out: &mut Vec<String>) {
        if self.buf.len() < MAX_OSC {
            if let Some(rest) = self.buf.strip_prefix(b"7;") {
                if let Some(p) = std::str::from_utf8(rest).ok().and_then(osc7_path) {
                    out.push(p);
                }
            }
        }
        self.buf.clear();
    }
}

/// OSC 7 的正文（`7;` 之后）→ 目录。只认 `file:` 协议，百分号解码（同 TS 版 `decodeURIComponent(pathname)`）
pub fn osc7_path(payload: &str) -> Option<String> {
    let u = url::Url::parse(payload).ok()?;
    if u.scheme() != "file" {
        return None;
    }
    // `file://host` 后面没有路径：url 会补一个 "/"，但那不是 shell 真的报的目录
    let after_host = payload.get("file://".len()..)?;
    if !after_host.contains('/') {
        return None;
    }
    let p = percent_encoding::percent_decode_str(u.path()).decode_utf8().ok()?.into_owned();
    (!p.is_empty()).then_some(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bel_and_st_terminators() {
        let mut s = Osc7Scanner::default();
        assert_eq!(s.feed(b"abc\x1b]7;file://mac/Users/me/p\x07def"), vec!["/Users/me/p".to_string()]);
        assert_eq!(s.feed(b"\x1b]7;file://mac/tmp/a%20b\x1b\\"), vec!["/tmp/a b".to_string()], "ESC \\ 结束 + 百分号解码");
        assert_eq!(s.feed(b"\x1b]7;file://mac/Users/%E4%B8%AD\x07"), vec!["/Users/中".to_string()], "中文目录");
    }

    #[test]
    fn split_across_reads() {
        let mut s = Osc7Scanner::default();
        let all = b"\x1b]7;file://h/Users/me/proj\x1b\\";
        let mut got = Vec::new();
        for chunk in all.chunks(3) {
            got.extend(s.feed(chunk));
        }
        assert_eq!(got, vec!["/Users/me/proj".to_string()], "一条 OSC 7 被切成好几次读");
    }

    #[test]
    fn ignores_other_osc_and_junk() {
        let mut s = Osc7Scanner::default();
        assert!(s.feed(b"\x1b]0;title\x07\x1b]2;x\x1b\\\x1b[31mred\x1b[0m").is_empty(), "标题等别的 OSC 不管");
        assert!(s.feed(b"\x1b]7;http://x/y\x07").is_empty(), "不是 file: 不认");
        assert!(s.feed(b"\x1b]7;notaurl\x07").is_empty());
        // 超长的坏 OSC 不会让缓冲无限涨，之后还能正常认
        let mut junk = b"\x1b]7;file://h/".to_vec();
        junk.extend(std::iter::repeat_n(b'a', 10_000));
        junk.push(0x07);
        assert!(s.feed(&junk).is_empty());
        assert_eq!(s.feed(b"\x1b]7;file://h/ok\x07"), vec!["/ok".to_string()]);
    }

    #[test]
    fn payload_parsing() {
        assert_eq!(osc7_path("file://host/Users/me"), Some("/Users/me".into()));
        assert_eq!(osc7_path("file:///Users/me"), Some("/Users/me".into()), "没有 host");
        assert_eq!(osc7_path("file://host"), None, "空路径不算");
    }
}
