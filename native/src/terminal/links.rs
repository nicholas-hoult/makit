//! 终端里的链接识别（#214）：本地路径、网址、OSC 8 超链接，以及双击选词。照 TS 版 `src/terminalLinks.ts` 移植。
//!
//! 为什么单独测（测试向量照搬 `scripts/test-terminal-links.ts`）：
//! - 网址以前根本不识别，还会被路径正则切走一截（`github.com/a/b` 当成相对路径），点了打不开；
//! - OSC 8 链接要把 `file://` 解成本地路径，还得拒绝 `javascript:` 这类协议 —— 错了就是安全问题；
//! - 链接位置要按「终端格子」算，而一个汉字占两格，前面有中文时下划线整体往左偏；
//! - 双击中文要按词选，这得在「终端格子」和「字符串」之间来回换算，宽字符算错就选歪。
//! 这些在界面上都得盯着终端才看得出来。
//!
//! 和 TS 版的两处口径差别（语言差异，行为不变）：
//! - 字符串下标用 **char 下标**（TS 是 UTF-16 单位）：emoji 在 TS 里占两个下标，这里占一个；
//! - `RowLink` 的行列从 0 起（TS 沿用 xterm 链接 API 的「从 1 起、end 含」），end 仍然含。
//! - JS 的 `\w` `\d` `\b` 是 ASCII 口径，Rust regex 默认 Unicode，所以正则里一律写成显式的 ASCII 集合。

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;

/// 双击选词的分隔符，照 对标终端 的 `selection-word-chars` 默认值（对标产品 的内核，#214）。
/// 双击 `foo.ts:12` 只选 `foo.ts`；`/` 和 `.` 故意不算，双击路径能整体选中。
pub const WORD_SEPARATORS: &str = " \t'\"│`|:;,()[]{}<>$";

/// 路径字符（JS 版的 `[\w一-龥.\-]`）
const W: &str = r"[A-Za-z0-9_\x{4e00}-\x{9fa5}.\-]";
const LC: &str = r"(?::[0-9]+(?::[0-9]+)?)?";

/// 匹配本地路径（支持中文文件名 + 目录）：
/// 1. 含扩展名的文件：foo.md、Lens智能选币器_TRD.md、src/bar.ts:12:34
/// 2. 绝对路径或 ~ 路径（含目录）：/Users/foo、~/Pictures
/// 3. 显式相对路径：./foo、../bar
/// 4. ls -F 风格目录（后缀 /）：Pictures/ —— JS 版用前瞻 `(?=\s|$)`，Rust regex 没有前瞻，
///    改成把那个空白一起吃进来、匹配后再去掉（`trim_lookahead`）
static PATH_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    let wc = r"[A-Za-z0-9_\x{4e00}-\x{9fa5}.\-/]";
    Regex::new(&format!(
        r"(?:\.{{0,2}}/|~/)?(?:{W}+/)*{W}+\.[A-Za-z0-9_]+{LC}|(?:/|~/){wc}*{W}{LC}|\.{{1,2}}/{wc}*{W}{LC}|{W}+/(?:\s|$)"
    ))
    .unwrap()
});
/// 路径以前是从 token 中间开始匹配的：`private/docs/issue` 被切成 `/docs/issue`，还当成绝对路径
static MID_TOKEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_\x{4e00}-\x{9fa5}.\-/~]$").unwrap());
/// 候选 token：按空白和常见包裹符号切
static TOKEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"[^\s'"`()\[\]{}<>,;|]+"#).unwrap());
/// 相对路径（至少一个斜杠），段里允许中文
static REL_WITH_SLASH: LazyLock<Regex> = LazyLock::new(|| Regex::new(&format!(r"^{W}+(?:/{W}+)+/?$")).unwrap());
/// 裸名字：只收 ASCII、至少一个字母（排除 42、&&），中文词太常见，不收
static BARE_NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_.\-]*[A-Za-z][A-Za-z0-9_.\-]*$").unwrap());
/// 网址：到空白、引号、尖括号、中文（含全角标点）为止。中文紧挨着网址是常态（「打开https://…看看」）。
/// `\b` 必须是 ASCII 口径：Unicode 口径下「开h」之间不算词边界，紧挨中文的网址就认不出来了
static URL_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?-u:\b)https?://[^\s<>"'`\x{3000}-\x{9fff}\x{ff00}-\x{ffef}]+"#).unwrap());
static URL_HEAD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^https?://.").unwrap());
static CJK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\p{Han}\p{Hiragana}\p{Katakana}\p{Hangul}]").unwrap());
static LINE_COL_SUFFIX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?::[0-9]+){1,2}$").unwrap());

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkKind {
    Url,
    Path,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoundLink {
    pub kind: LinkKind,
    /// char 下标，`end` 不含
    pub start: usize,
    pub end: usize,
    pub text: String,
    /// 只是候选、要查磁盘才算数（#214）：不带斜杠结尾的相对目录（`src/components`）、裸名字（`ls` 输出里的 `docs`）。
    /// 这些写法和普通单词、`and/or` 长得一样，字面上分不出来，存在才算链接。
    pub verify: bool,
}

/// 排除明显的伪路径（版本号、slash 命令、纯数字等）
pub fn is_likely_path(text: &str) -> bool {
    // 全数字+点（版本号 1.14.1 / IP 地址）
    if !text.is_empty() && text.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return false;
    }
    // 长度太短（<3 字符）
    if text.chars().count() < 3 {
        return false;
    }
    // /xxx 形式但 xxx 没有任何斜杠也没有扩展名（slash 命令如 /reload-plugins）
    if let Some(rest) = text.strip_prefix('/') {
        if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
            return false;
        }
    }
    true
}

/// 解析路径文本：剥离行号 / 列号 → (路径, 行, 列)
pub fn parse_path_text(text: &str) -> (String, Option<u32>, Option<u32>) {
    static RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(.+?)(?::([0-9]+)(?::([0-9]+))?)?$").unwrap());
    match RE.captures(text) {
        Some(c) => (
            c[1].to_string(),
            c.get(2).and_then(|m| m.as_str().parse().ok()),
            c.get(3).and_then(|m| m.as_str().parse().ok()),
        ),
        None => (text.to_string(), None, None),
    }
}

/// 把相对路径按 cwd 解析成绝对路径。`~` 原样留着，交给 core 的 paths_exist / open_path 展开
pub fn resolve_path(text: &str, cwd: &str) -> String {
    if text.starts_with('/') || text.starts_with("~/") || text == "~" {
        return text.to_string();
    }
    let base = cwd.trim_end_matches('/');
    if let Some(rest) = text.strip_prefix("./") {
        return format!("{base}/{rest}");
    }
    if text.starts_with("../") {
        let mut parts: Vec<&str> = base.split('/').collect();
        let mut rest = text;
        while let Some(r) = rest.strip_prefix("../") {
            parts.pop();
            rest = r;
        }
        return format!("{}/{rest}", parts.join("/"));
    }
    format!("{base}/{text}")
}

/// 去掉网址尾巴上的句读和不配对的右括号：「(见 https://x.com/a).」里的 `).` 不属于网址，维基那种配对的括号要留
fn trim_url(u: &str) -> &str {
    let mut s = u;
    while let Some(last) = s.chars().last() {
        if ".,;:!?'\"".contains(last) {
            s = &s[..s.len() - last.len_utf8()];
            continue;
        }
        let open = match last {
            ')' => Some('('),
            ']' => Some('['),
            '}' => Some('{'),
            _ => None,
        };
        if let Some(open) = open {
            if s.matches(last).count() > s.matches(open).count() {
                s = &s[..s.len() - 1];
                continue;
            }
        }
        break;
    }
    s
}

/// 在一行文字里找网址和本地路径，返回 char 下标（`end` 不含）。
/// 网址优先：和网址重叠的路径匹配丢掉 —— 以前没有网址识别，`github.com/a/b` 会被当成相对路径，点了打不开。
pub fn find_links(text: &str) -> Vec<FoundLink> {
    // 字节下标 → char 下标
    let mut char_at: Vec<usize> = vec![0; text.len() + 1];
    let mut ci = 0;
    for (bi, ch) in text.char_indices() {
        for k in 0..ch.len_utf8() {
            char_at[bi + k] = ci;
        }
        ci += 1;
    }
    char_at[text.len()] = ci;

    let mut out: Vec<FoundLink> = Vec::new();
    for m in URL_REGEX.find_iter(text) {
        let t = trim_url(m.as_str());
        if URL_HEAD.is_match(t) {
            out.push(FoundLink {
                kind: LinkKind::Url,
                start: char_at[m.start()],
                end: char_at[m.start() + t.len()],
                text: t.to_string(),
                verify: false,
            });
        }
    }
    let overlaps = |out: &[FoundLink], s: usize, e: usize| out.iter().any(|l| s < l.end && e > l.start);

    // 确定的路径：带扩展名、绝对 / ~ / ./ 开头、结尾带斜杠的目录
    for m in PATH_REGEX.find_iter(text) {
        let raw = m.as_str().trim_end_matches(|c: char| c.is_whitespace());
        let (bs, be) = (m.start(), m.start() + raw.len());
        if !is_likely_path(raw) {
            continue;
        }
        // 从 token 中间开始的不算
        if let Some(prev) = text[..bs].chars().last() {
            if MID_TOKEN.is_match(prev.encode_utf8(&mut [0u8; 4])) {
                continue;
            }
        }
        let (s, e) = (char_at[bs], char_at[be]);
        if overlaps(&out, s, e) {
            continue;
        }
        out.push(FoundLink { kind: LinkKind::Path, start: s, end: e, text: raw.to_string(), verify: false });
    }

    // 候选：相对目录、裸名字，要查磁盘
    for t in TOKEN.find_iter(text) {
        let raw = t.as_str().trim_end_matches(['.', ':']);
        if raw.chars().count() < 2 || raw.chars().all(|c| c.is_ascii_digit() || "/.:-".contains(c)) {
            continue;
        }
        if !REL_WITH_SLASH.is_match(raw) && !BARE_NAME.is_match(raw) {
            continue;
        }
        let (s, e) = (char_at[t.start()], char_at[t.start() + raw.len()]);
        if overlaps(&out, s, e) {
            continue;
        }
        out.push(FoundLink { kind: LinkKind::Path, start: s, end: e, text: raw.to_string(), verify: true });
    }
    out.sort_by_key(|l| l.start);
    out
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Osc8Target {
    Url(String),
    File(String),
}

/// OSC 8 超链接（程序主动嵌在输出里的链接）指向哪。只放行 http / https 和 `file://`，
/// 其余（`javascript:` 之类）一律不认 —— 这是程序输出里带来的地址，不能什么都拿去打开。
pub fn osc8_target(uri: &str) -> Option<Osc8Target> {
    let u = url::Url::parse(uri).ok()?;
    match u.scheme() {
        "http" | "https" => Some(Osc8Target::Url(uri.to_string())),
        "file" => {
            let path = percent_encoding::percent_decode_str(u.path()).decode_utf8().ok()?.into_owned();
            (!path.is_empty()).then_some(Osc8Target::File(path))
        }
        _ => None,
    }
}

/// 终端一行里的一个格子：宽字符（汉字、emoji）占两格，第二格 `width == 0`、没有字
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cell {
    pub chars: String,
    pub width: u8,
}

impl Cell {
    pub fn new(chars: impl Into<String>, width: u8) -> Self {
        Self { chars: chars.into(), width }
    }
}

/// 把一行格子拼成字符串，同时给出「字符串第 i 个 char 在第几格」。
/// 以前链接位置直接拿字符串下标当列号，一个汉字占两格，于是前面有中文时下划线整体往左偏。
pub fn cells_to_text(cells: &[Cell]) -> (String, Vec<usize>) {
    let mut text = String::new();
    let mut col_of = Vec::new();
    for (col, c) in cells.iter().enumerate() {
        if c.width == 0 {
            continue;
        }
        let ch = if c.chars.is_empty() { " " } else { c.chars.as_str() };
        for k in ch.chars() {
            text.push(k);
            col_of.push(col);
        }
    }
    (text, col_of)
}

/// 分词器：输入一段文字，返回 (起始 char 下标, 词) 列表。真实现见 `segment.rs`（macOS 系统分词）
pub type Segmenter<'a> = &'a dyn Fn(&str) -> Vec<(usize, String)>;

/// 双击中文时按词选（#214）：点到的是中日韩文字，就用分词器找出它所在的词，返回要选中的 (起始格, 格数)；
/// 点到别的返回 None，交回按分隔符的默认规则。分词器注入进来，测试里用固定切法。
pub fn cjk_word_range(cells: &[Cell], col: usize, segment: Segmenter) -> Option<(usize, usize)> {
    let mut base = col;
    if base > 0 && cells.get(base).map(|c| c.width == 0).unwrap_or(false) {
        base -= 1; // 点在宽字符的右半格
    }
    let cell = cells.get(base)?;
    if !CJK.is_match(&cell.chars) {
        return None;
    }
    let (text, col_of) = cells_to_text(cells);
    let i = col_of.iter().position(|&c| c == base)?;
    let (idx, seg) = segment(&text).into_iter().find(|(idx, seg)| i >= *idx && i < idx + seg.chars().count())?;
    let start_col = col_of[idx];
    let last_col = col_of[idx + seg.chars().count() - 1];
    let w = cells.get(last_col).map(|c| c.width).filter(|&w| w > 0).unwrap_or(1) as usize;
    Some((start_col, last_col + w - start_col))
}

/// 一行终端的格子，`y` 是缓冲区里的行号
#[derive(Clone, Debug)]
pub struct Row {
    pub y: i32,
    pub cells: Vec<Cell>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowLink {
    pub kind: LinkKind,
    pub text: String,
    pub verify: bool,
    /// (列, 行)，从 0 起；end 含
    pub start: (usize, i32),
    pub end: (usize, i32),
    /// 在拼接后的格子序列里从第几格开始、占几格（给双击选中用）
    pub start_cell: usize,
    pub cells: usize,
}

/// 在「一整行逻辑行」（被折成几行显示的那一行，按顺序传进来）里找链接，位置按格子换算回行、列。
/// 以前逐行识别，折到下一行的网址被当成两段，哪段都不认（用户 2026-09-25 反馈）。
pub fn row_links(rows: &[Row]) -> Vec<RowLink> {
    let mut cells: Vec<Cell> = Vec::new();
    let mut pos: Vec<(usize, i32)> = Vec::new();
    for r in rows {
        for (x, c) in r.cells.iter().enumerate() {
            cells.push(c.clone());
            pos.push((x, r.y));
        }
    }
    let (text, col_of) = cells_to_text(&cells);
    find_links(&text)
        .into_iter()
        .filter(|l| l.end > l.start)
        .map(|l| {
            let start_cell = col_of[l.start];
            let last_cell = col_of[l.end - 1];
            let w = cells.get(last_cell).map(|c| c.width).filter(|&w| w > 0).unwrap_or(1) as usize;
            let end_cell = last_cell + w - 1;
            RowLink {
                kind: l.kind,
                text: l.text,
                verify: l.verify,
                start: pos[start_cell],
                end: pos[end_cell.min(pos.len() - 1)],
                start_cell,
                cells: end_cell - start_cell + 1,
            }
        })
        .collect()
}

/// 双击落在链接（网址或路径）上时，选中整个链接，跨几行都行（用户 2026-09-25 反馈：折行的网址双击选不中）。
/// `col`、`row` 是被点的格子（row 是缓冲区行号）；返回 (起始列, 起始行, 格数)。
/// 不在链接上返回 None，交回别的规则（中文按词、其余按分隔符）。
pub fn link_selection_at(rows: &[Row], col: usize, row: i32) -> Option<(usize, i32, usize)> {
    let mut offset = 0;
    let mut hit: Option<usize> = None;
    for r in rows {
        if r.y == row {
            let mut h = offset + col;
            if col > 0 && r.cells.get(col).map(|c| c.width == 0).unwrap_or(false) {
                h -= 1;
            }
            hit = Some(h);
            break;
        }
        offset += r.cells.len();
    }
    let hit = hit?;
    // 只认确定的链接：候选（裸名字、相对目录）不查磁盘分不出是不是路径，双击它们交给按分隔符选
    // —— `/` 不算分隔符，`src/components` 照样整个选中
    let link = row_links(rows).into_iter().find(|l| !l.verify && hit >= l.start_cell && hit < l.start_cell + l.cells)?;
    // 路径选到文件名为止，不带末尾的 `:行号:列号`：双击是为了拿到能用的路径，`foo.ts:12` 贴进 cd / open 都不认。
    // 行列号都是 ASCII，一个字符占一格，直接按字符数扣
    let suffix = match link.kind {
        LinkKind::Path => LINE_COL_SUFFIX.find(&link.text).map(|m| m.as_str().len()).unwrap_or(0),
        LinkKind::Url => 0,
    };
    Some((link.start.0, link.start.1, link.cells - suffix))
}

/// 候选路径存在与否的缓存：悬停一行就要查一批，5 秒内不重复扫盘；目录刚建 / 刚删的，最多晚 5 秒反映出来。
/// 超过 2000 条整个清空（同 TS 版 terminalLinkInstall.ts）。时间由调用方传（毫秒），测试里好造
#[derive(Default)]
pub struct ExistCache {
    map: HashMap<String, (bool, f64)>,
}

pub const EXIST_TTL_MS: f64 = 5000.0;
pub const EXIST_CACHE_MAX: usize = 2000;

impl ExistCache {
    /// 返回这批路径里哪些存在。`check` 是真查磁盘（一次最多 200 个，见 core `paths_exist`）
    pub fn existing(&mut self, paths: &[String], now_ms: f64, check: impl FnOnce(Vec<String>) -> Vec<bool>) -> Vec<bool> {
        if self.map.len() > EXIST_CACHE_MAX {
            self.map.clear();
        }
        let mut need: Vec<String> = Vec::new();
        for p in paths {
            let fresh = self.map.get(p).map(|(_, at)| now_ms - at <= EXIST_TTL_MS).unwrap_or(false);
            if !fresh && !need.contains(p) {
                need.push(p.clone());
            }
        }
        if !need.is_empty() {
            let res = check(need.clone());
            for (i, p) in need.into_iter().enumerate() {
                self.map.insert(p, (res.get(i).copied().unwrap_or(false), now_ms));
            }
        }
        paths.iter().map(|p| self.map.get(p).map(|(ok, _)| *ok).unwrap_or(false)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 只列**确定**的链接；要查磁盘的候选（verify）不是链接，由后面「目录」那组用 cand() 单独测
    fn texts(s: &str) -> Vec<String> {
        find_links(s)
            .into_iter()
            .filter(|l| !l.verify)
            .map(|l| format!("{}:{}", if l.kind == LinkKind::Url { "url" } else { "path" }, l.text))
            .collect()
    }
    fn cand(s: &str) -> Vec<String> {
        find_links(s).into_iter().map(|l| format!("{}{}", l.text, if l.kind == LinkKind::Path && l.verify { "?" } else { "" })).collect()
    }
    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn urls() {
        assert_eq!(texts("see https://github.com/a/b and src/foo.ts:12"), v(&["url:https://github.com/a/b", "path:src/foo.ts:12"]), "网址和路径各认各的");
        assert_eq!(texts("https://example.com/docs/x.md"), v(&["url:https://example.com/docs/x.md"]), "网址不会再被路径正则切走一截");
        assert_eq!(texts("(见 https://x.com/a)."), v(&["url:https://x.com/a"]), "句尾标点和外层括号不算进网址");
        assert_eq!(texts("https://en.wikipedia.org/wiki/A_(b)"), v(&["url:https://en.wikipedia.org/wiki/A_(b)"]), "网址里配对的括号保留");
        assert_eq!(texts("打开https://x.com/a看看"), v(&["url:https://x.com/a"]), "紧挨着中文也能切干净");
        let pos: Vec<(usize, usize)> = find_links("x https://a.io y").iter().map(|l| (l.start, l.end)).collect();
        assert_eq!(pos, vec![(2, 14)], "位置是字符串下标（https://a.io 共 12 个字符）");
    }

    #[test]
    fn local_paths() {
        assert_eq!(texts("cd /Users/me/proj"), v(&["path:/Users/me/proj"]), "绝对路径");
        assert_eq!(texts("open ~/Pictures"), v(&["path:~/Pictures"]), "~ 路径");
        assert_eq!(texts("./a/b"), v(&["path:./a/b"]), "显式相对路径");
        assert_eq!(texts("Pictures/"), v(&["path:Pictures/"]), "ls -F 风格目录");
        assert_eq!(texts("看 设计稿_v2.md"), v(&["path:设计稿_v2.md"]), "中文文件名");
        assert_eq!(texts("v 1.14.1"), Vec::<String>::new(), "版本号不是路径");
        assert_eq!(texts("/reload-plugins"), Vec::<String>::new(), "slash 命令不是路径");
    }

    #[test]
    fn osc8() {
        assert_eq!(osc8_target("https://a.com/x"), Some(Osc8Target::Url("https://a.com/x".into())));
        assert_eq!(osc8_target("file:///Users/me/a%20b.ts"), Some(Osc8Target::File("/Users/me/a b.ts".into())), "file:// 解成本地路径（含转义）");
        assert_eq!(osc8_target("file://myhost/Users/me/x"), Some(Osc8Target::File("/Users/me/x".into())), "file:// 带主机名");
        assert_eq!(osc8_target("javascript:alert(1)"), None, "拒绝 javascript: 协议");
        assert_eq!(osc8_target("not a uri"), None, "拒绝乱写的");
    }

    fn c(s: &str, w: u8) -> Cell {
        Cell::new(s, w)
    }
    /// 一个汉字占两格（第二格是宽度 0 的占位）
    fn zh(s: &str) -> Vec<Cell> {
        s.chars()
            .flat_map(|ch| {
                let wide = ('\u{3000}'..='\u{9fff}').contains(&ch) || ('\u{ff00}'..='\u{ffef}').contains(&ch);
                if wide { vec![c(&ch.to_string(), 2), c("", 0)] } else { vec![c(&ch.to_string(), 1)] }
            })
            .collect()
    }

    #[test]
    fn cells_and_text() {
        assert_eq!(cells_to_text(&zh("打开 a")), ("打开 a".to_string(), vec![0, 2, 4, 5]));
        // TS 版这里是 [0, 0, 2]（emoji 占两个 UTF-16 单位）；Rust 按 char 算是一个下标
        assert_eq!(cells_to_text(&[c("😀", 2), c("", 0), c("a", 1)]), ("😀a".to_string(), vec![0, 2]));
    }

    fn fake_seg(s: &str) -> Vec<(usize, String)> {
        let words = ["打开", "滚动条", "拖不动", " ", "x"];
        let chars: Vec<char> = s.chars().collect();
        let mut out = Vec::new();
        let mut i = 0;
        for w in words {
            let wc: Vec<char> = w.chars().collect();
            let found = (i..=chars.len().saturating_sub(wc.len())).find(|&j| chars[j..].starts_with(&wc));
            if let Some(j) = found {
                out.push((j, w.to_string()));
                i = j + wc.len();
            }
        }
        out
    }

    #[test]
    fn cjk_double_click() {
        let line = zh("打开滚动条拖不动 x");
        // 「滚动条」从第 4 格开始（打开占 0~3），共 3 个字 = 6 格；点「动」（第 6 格）
        assert_eq!(cjk_word_range(&line, 6, &fake_seg), Some((4, 6)), "点中文词里任意一格，选中整个词");
        assert_eq!(cjk_word_range(&line, 7, &fake_seg), Some((4, 6)), "点在汉字的右半格（占位格）也一样");
        assert_eq!(cjk_word_range(&line, 17, &fake_seg), None, "点英文不管，交给默认规则");
    }

    #[test]
    fn separators_are_reference() {
        assert_eq!(WORD_SEPARATORS, " \t'\"│`|:;,()[]{}<>$");
    }

    fn row(y: i32, s: &str) -> Row {
        Row { y, cells: format!("{s:<10}").chars().map(|ch| c(&ch.to_string(), 1)).collect() }
    }

    #[test]
    fn wrapped_url_is_one_link() {
        // 终端 10 列宽；「see https://a.io/abcdef x」折成三行
        let wrapped = vec![row(5, "see https:"), row(6, "//a.io/abc"), row(7, "def x")];
        let links = row_links(&wrapped);
        let urls: Vec<&RowLink> = links.iter().filter(|l| !l.verify).collect();
        assert_eq!(urls.len(), 1);
        assert_eq!(urls[0].text, "https://a.io/abcdef", "折行的网址认成一整个");
        // 起点在第 5 行第 4 格，终点在第 7 行第 2 格（0 起；TS 版 1 起是 (5,6)~(3,8)）
        assert_eq!((urls[0].start, urls[0].end), ((4, 5), (2, 7)));
        assert_eq!(link_selection_at(&wrapped, 3, 6), Some((4, 5, 19)), "双击第二行那截");
        assert_eq!(link_selection_at(&wrapped, 6, 5), Some((4, 5, 19)), "双击第一行那截（https: 的冒号以前是分隔符，只能选一半）");
        assert_eq!(link_selection_at(&wrapped, 1, 5), None, "双击非链接处不管");
    }

    #[test]
    fn path_double_click_drops_line_col() {
        let path_row = vec![row(3, "foo.ts:12 ")];
        assert_eq!(link_selection_at(&path_row, 1, 3), Some((0, 3, 6)), "双击 foo.ts:12 的 foo 只选 foo.ts");
        assert_eq!(link_selection_at(&[row(0, "a/b.ts:3:4")], 2, 0), Some((0, 0, 6)), "双击带行列号的深路径，选到文件名为止");
        assert_eq!(link_selection_at(&path_row, 7, 3), Some((0, 3, 6)), "点在 :12 上也按路径算，同样不带行号");
    }

    #[test]
    fn directory_candidates() {
        assert_eq!(cand("cd src/components && ls"), v(&["cd?", "src/components?", "ls?"]), "相对目录（不带斜杠结尾）是候选");
        assert_eq!(cand("private/docs/issue"), v(&["private/docs/issue?"]), "多级相对目录不再从中间切");
        assert_eq!(cand("docs  scripts  src"), v(&["docs?", "scripts?", "src?"]), "ls 输出的裸目录名是候选");
        assert_eq!(cand("修改了 src/components 下的文件"), v(&["src/components?"]), "中文句子里的相对目录");
        assert_eq!(cand("1/2 2026/09/25 42"), Vec::<String>::new(), "纯数字和分数、日期不当候选");
        assert_eq!(cand("/Users/me/proj ~/a ./b c.ts lib/"), v(&["/Users/me/proj", "~/a", "./b", "c.ts", "lib/"]), "确定的路径不用查磁盘");
        assert_eq!(cand("https://a.com/src/x"), v(&["https://a.com/src/x"]), "网址里的片段不当候选");
    }

    #[test]
    fn resolve_and_parse() {
        assert_eq!(resolve_path("./a", "/x/y/"), "/x/y/a");
        assert_eq!(resolve_path("../../a", "/x/y/z"), "/x/a");
        assert_eq!(resolve_path("src/a", "/x"), "/x/src/a");
        assert_eq!(resolve_path("~/a", "/x"), "~/a");
        assert_eq!(resolve_path("/abs", "/x"), "/abs");
        assert_eq!(parse_path_text("a/b.ts:3:4"), ("a/b.ts".into(), Some(3), Some(4)));
        assert_eq!(parse_path_text("a.ts"), ("a.ts".into(), None, None));
    }

    #[test]
    fn exist_cache_ttl_and_cap() {
        let mut cache = ExistCache::default();
        let calls = std::cell::Cell::new(0);
        let check = |ps: Vec<String>| {
            calls.set(calls.get() + ps.len());
            ps.iter().map(|p| p == "/a").collect::<Vec<_>>()
        };
        assert_eq!(cache.existing(&v(&["/a", "/b", "/a"]), 0.0, check), vec![true, false, true]);
        assert_eq!(cache.existing(&v(&["/a", "/b"]), 4999.0, check), vec![true, false], "5 秒内走缓存");
        assert_eq!(calls.get(), 2, "同一批里重复的只查一次，5 秒内不再查");
        cache.existing(&v(&["/a"]), 5001.0, check);
        assert_eq!(calls.get(), 3, "过期重查");
    }
}
