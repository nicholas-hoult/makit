//! 中文分词（双击中文按词选，#214）：用 macOS 自带的 `CFStringTokenizer`（按词切，zh 语言）。
//!
//! 取舍：TS 版用 WebKit 的 `Intl.Segmenter("zh", { granularity: "word" })`，底层是系统 ICU 的词典分词。
//! Rust 这边的候选：
//! - `jieba-rs`：最成熟的 Rust 中文分词，但要把几 MB 的词典编进二进制（GPUI 版现在整个才 7MB），切法也和现在不一样；
//! - `CFStringTokenizer`：系统框架，零体积，和 Safari / WebKit 同一套系统词典（`kCFStringTokenizerUnitWord`），
//!   切法最接近现在的 Tauri 版。core-foundation-sys 已经在依赖树里（GPUI 用它）。
//! 选后者。非 macOS 平台返回空（= 退回按分隔符选），三端时（#184）再换 ICU4X。
//!
//! 为什么单独测：UTF-16 区间 ↔ char 下标换算错了，双击就会选歪一两个字；真分词器的切法不固定，
//! 测试只要求「选中范围包住所点的字」（同 TS 版的真分词器用例）。

/// 返回 (起始 char 下标, 词)
pub fn segment_words(text: &str) -> Vec<(usize, String)> {
    #[cfg(target_os = "macos")]
    {
        mac::segment(text)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = text;
        Vec::new()
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use core_foundation_sys::base::{kCFAllocatorDefault, CFIndex, CFRange, CFRelease};
    use core_foundation_sys::locale::CFLocaleCreate;
    use core_foundation_sys::string::{kCFStringEncodingUTF8, CFStringCreateWithBytes, CFStringRef};
    use core_foundation_sys::string_tokenizer::{
        kCFStringTokenizerTokenNone, kCFStringTokenizerUnitWord, CFStringTokenizerAdvanceToNextToken,
        CFStringTokenizerCreate, CFStringTokenizerGetCurrentTokenRange,
    };

    fn cfstr(s: &str) -> CFStringRef {
        unsafe { CFStringCreateWithBytes(kCFAllocatorDefault, s.as_ptr(), s.len() as CFIndex, kCFStringEncodingUTF8, 0) }
    }

    pub fn segment(text: &str) -> Vec<(usize, String)> {
        let chars: Vec<char> = text.chars().collect();
        // UTF-16 下标 → char 下标
        let mut char_of_u16 = Vec::with_capacity(text.len() + 1);
        for (i, c) in chars.iter().enumerate() {
            for _ in 0..c.len_utf16() {
                char_of_u16.push(i);
            }
        }
        char_of_u16.push(chars.len());
        let mut out = Vec::new();
        unsafe {
            let s = cfstr(text);
            if s.is_null() {
                return out;
            }
            let loc_id = cfstr("zh");
            let locale = CFLocaleCreate(kCFAllocatorDefault, loc_id);
            let range = CFRange { location: 0, length: (char_of_u16.len() - 1) as CFIndex };
            let tok = CFStringTokenizerCreate(kCFAllocatorDefault, s, range, kCFStringTokenizerUnitWord, locale);
            if !tok.is_null() {
                while CFStringTokenizerAdvanceToNextToken(tok) != kCFStringTokenizerTokenNone {
                    let r = CFStringTokenizerGetCurrentTokenRange(tok);
                    let (a, b) = (r.location as usize, (r.location + r.length) as usize);
                    if b > a && b < char_of_u16.len() + 1 {
                        let (ca, cb) = (char_of_u16[a], char_of_u16[b.min(char_of_u16.len() - 1)]);
                        if cb > ca {
                            out.push((ca, chars[ca..cb].iter().collect()));
                        }
                    }
                }
                CFRelease(tok as _);
            }
            if !locale.is_null() {
                CFRelease(locale as _);
            }
            CFRelease(loc_id as _);
            CFRelease(s as _);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::links::{cjk_word_range, Cell};

    fn zh(s: &str) -> Vec<Cell> {
        s.chars()
            .flat_map(|ch| if ch.is_ascii() { vec![Cell::new(ch.to_string(), 1)] } else { vec![Cell::new(ch.to_string(), 2), Cell::new("", 0)] })
            .collect()
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn real_segmenter_covers_clicked_char() {
        let line = zh("打开滚动条拖不动 x");
        let r = cjk_word_range(&line, 6, &segment_words).expect("点「动」应该落在某个词里");
        assert!(r.0 <= 6 && r.0 + r.1 > 6, "选中范围 {r:?} 要包住第 6 格");
        assert!(r.1 >= 2 && r.1 % 2 == 0, "选中的是整字（宽字符按 2 格算）");
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn indexes_are_chars_not_utf16() {
        // emoji 占两个 UTF-16 单位；后面的中文词下标要按 char 算
        let words = segment_words("😀 中文分词");
        let (i, w) = words.iter().find(|(_, w)| w.contains('中')).expect("中文要切出来");
        assert_eq!(*i, 2, "{words:?}");
        assert!(w.starts_with('中'));
    }
}
