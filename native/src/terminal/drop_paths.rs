//! 从 Finder 拖文件进终端 → 在命令行里插入文件路径（TS 版 `src/dropPaths.ts`，测试向量照搬 `test-drop-paths.ts`）。
//!
//! GPUI 的 `ExternalPaths` 直接给 `PathBuf`，不用像 WebView 那样从 `text/uri-list` 解 `file://`；
//! 但解析函数照样移植（将来从粘贴板 / 别的来源拿到 URL 时用），引号和拼接规则是实际在用的。
//!
//! 为什么单独测：两类边界 —— 百分号解码、要不要加引号。错了不是「不好看」，而是往用户的命令行里插进一个错的东西。
//! 引号**刻意用黑名单**：非 ASCII 在 shell 里就是普通字符，白名单会把每一个中文文件名都套上引号。

/// 单行 → 绝对路径，不是文件就返回 None（拖网页链接进来也会给 uri-list，http 不能当路径插进去）
pub fn file_url_to_path(raw: &str) -> Option<String> {
    // uri-list 按 RFC 2483 是 CRLF 分隔，行尾的 \r 得吃掉
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    if s.starts_with('/') {
        return Some(s.to_string());
    }
    if s.len() < 7 || !s[..7].eq_ignore_ascii_case("file://") {
        return None;
    }
    let mut rest = &s[7..];
    // file:///path 和 file://localhost/path 都合法，后者要把 host 摘掉
    if rest.len() >= 10 && rest[..10].eq_ignore_ascii_case("localhost/") {
        rest = &rest[9..];
    }
    if !rest.starts_with('/') {
        return None;
    }
    decode_uri_component(rest)
}

/// 同 JS `decodeURIComponent`：坏的百分号编码（`%zz`、截断的 `%E6`、解出来不是 UTF-8）一律失败 ——
/// 宁可不插也不要插一个半解码的路径
fn decode_uri_component(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// 按 POSIX 规则给路径加引号：含 shell 元字符（空白、`|&;<>()$\`\\"'*?[]{}~!#`）就整体套单引号，
/// 单引号本身写成 `'\''`
pub fn quote_for_shell(path: &str) -> String {
    let special = |c: char| c.is_whitespace() || "|&;<>()$`\\\"'*?[]{}~!#".contains(c);
    if !path.chars().any(special) {
        return path.to_string();
    }
    format!("'{}'", path.replace('\'', "'\\''"))
}

/// 从拖拽数据里把所有文件路径抠出来：先 `text/uri-list`（跳过 `#` 注释、兼容 CRLF），
/// 什么都没有才看 `text/plain`；按路径去重
pub fn parse_dropped_paths(uri_list: &str, plain: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut collect = |blob: &str, out: &mut Vec<String>| {
        for line in blob.split('\n') {
            // RFC 2483：# 开头是注释
            if line.trim().starts_with('#') {
                continue;
            }
            if let Some(p) = file_url_to_path(line) {
                if !out.contains(&p) {
                    out.push(p);
                }
            }
        }
    };
    collect(uri_list, &mut out);
    // uri-list 是首选；只有它什么都没给出来时才看 text/plain，避免同一批文件解析两遍
    if out.is_empty() {
        collect(plain, &mut out);
    }
    out
}

/// 拼成要写进 PTY 的文本：各自判断引号、空格连接、末尾一个空格（Terminal.app / iTerm2 惯例）。空 → 空串
pub fn format_paths_for_terminal(paths: &[String]) -> String {
    if paths.is_empty() {
        return String::new();
    }
    paths.iter().map(|p| quote_for_shell(p)).collect::<Vec<_>>().join(" ") + " "
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Option<String> {
        Some(s.to_string())
    }

    #[test]
    fn file_urls() {
        assert_eq!(file_url_to_path("file:///Users/me/a.txt"), p("/Users/me/a.txt"));
        assert_eq!(file_url_to_path("file:///Users/me/My%20File.txt"), p("/Users/me/My File.txt"), "空格是 %20，必须解码");
        assert_eq!(file_url_to_path("file:///Users/me/%E6%96%87%E6%A1%A3.txt"), p("/Users/me/文档.txt"), "中文文件名");
        assert_eq!(file_url_to_path("file://localhost/Users/me/a.txt"), p("/Users/me/a.txt"), "file://localhost/… 也合法");
        assert_eq!(file_url_to_path("file:///a.txt\r"), p("/a.txt"), "行尾的 CRLF 要吃掉");
        assert_eq!(file_url_to_path("/Users/me/a.txt"), p("/Users/me/a.txt"), "有些来源直接给裸路径");
        assert_eq!(file_url_to_path("https://example.com/x"), None, "http 链接不是路径");
        assert_eq!(file_url_to_path("  "), None, "空串");
        assert_eq!(file_url_to_path("file://"), None, "file:// 后面没有路径");
        assert_eq!(file_url_to_path("file:///a%zz"), None, "坏的百分号编码返回 None");
    }

    #[test]
    fn quoting() {
        assert_eq!(quote_for_shell("/Users/me/a.txt"), "/Users/me/a.txt", "普通路径不加引号");
        assert_eq!(quote_for_shell("/Users/me/My File.txt"), "'/Users/me/My File.txt'", "带空格必须加引号");
        assert_eq!(quote_for_shell("/Users/me/文档.txt"), "/Users/me/文档.txt", "中文路径不加引号");
        assert_eq!(quote_for_shell("/Users/me/it's.txt"), "'/Users/me/it'\\''s.txt'", "单引号用 '\\'' 拼接");
        assert_eq!(quote_for_shell("/Users/me/a$b"), "'/Users/me/a$b'");
        assert_eq!(quote_for_shell("/Users/me/a&b"), "'/Users/me/a&b'");
        assert_eq!(quote_for_shell("/Users/me/a!b"), "'/Users/me/a!b'", "! 是交互式 shell 的历史展开");
        assert_eq!(quote_for_shell("/Users/me/a*b"), "'/Users/me/a*b'");
        assert_eq!(quote_for_shell("/Users/me/a\nb"), "'/Users/me/a\nb'", "换行也是空白");
        assert_eq!(quote_for_shell("/Users/me/a-b_c.1,2@3=4"), "/Users/me/a-b_c.1,2@3=4", "这些常见字符不必加引号");
    }

    #[test]
    fn parse_and_format() {
        let v = |xs: &[&str]| xs.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(parse_dropped_paths("file:///a.txt\r\nfile:///b.txt", ""), v(&["/a.txt", "/b.txt"]));
        assert_eq!(parse_dropped_paths("# comment\r\nfile:///a.txt", ""), v(&["/a.txt"]), "# 开头是注释");
        assert_eq!(parse_dropped_paths("", "/Users/me/a.txt"), v(&["/Users/me/a.txt"]), "uri-list 空时退回 text/plain");
        assert_eq!(parse_dropped_paths("file:///a.txt\nfile:///a.txt", ""), v(&["/a.txt"]), "去重");
        assert_eq!(parse_dropped_paths("", ""), Vec::<String>::new());
        assert_eq!(parse_dropped_paths("https://example.com/", ""), Vec::<String>::new(), "网页链接 → 空");
        assert_eq!(format_paths_for_terminal(&v(&["/a.txt"])), "/a.txt ");
        assert_eq!(format_paths_for_terminal(&v(&["/a.txt", "/b.txt"])), "/a.txt /b.txt ");
        assert_eq!(format_paths_for_terminal(&v(&["/a b.txt", "/c.txt"])), "'/a b.txt' /c.txt ");
        assert_eq!(format_paths_for_terminal(&[]), "");
    }
}
