//! ⌘F 搜索条的计数文本（照 `src/searchCount.ts`）。
//!
//! 值得单独测的原因全在边界上：终端给的结果下标是 **0-based**，而且匹配数超过高亮上限时下标是 **-1**。
//! 任一处理错，用户看到的就是「0/17」这种差一错误，或者一个匹配上千次的搜索显示「0/1234」。

use crate::ts;
/// 终端报上来的搜索进度（A 包通过 `search_bar::report_progress` 推过来）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SearchProgress {
    /// 当前停在第几个（0-based）；说不出来时 -1
    pub index: i64,
    pub count: usize,
}

/// 搜索词为空 / 还没收到过结果 → None（刚打开搜索条时不先闪一个「无结果」）
pub fn format_search_count(term: &str, p: Option<SearchProgress>) -> Option<String> {
    if term.trim().is_empty() {
        return None;
    }
    let p = p?;
    // count==0 必须排在 index<0 前面：没有匹配时两个条件同时成立
    if p.count == 0 {
        return Some(ts!("search.no_results").into());
    }
    if p.index < 0 || p.index as usize >= p.count {
        return Some(ts!("search.results", n = p.count));
    }
    Some(format!("{}/{}", p.index + 1, p.count))
}

#[cfg(test)]
mod tests {
    //! 向量照搬 `scripts/test-search-count.ts`
    use super::*;

    fn p(index: i64, count: usize) -> Option<SearchProgress> {
        Some(SearchProgress { index, count })
    }

    #[test]
    fn matches_ts_vectors() {
        assert_eq!(format_search_count("foo", p(0, 17)).as_deref(), Some("1/17"), "0-based → 1-based");
        assert_eq!(format_search_count("foo", p(4, 17)).as_deref(), Some("5/17"));
        assert_eq!(format_search_count("foo", p(16, 17)).as_deref(), Some("17/17"));
        assert_eq!(format_search_count("foo", p(0, 1)).as_deref(), Some("1/1"));
        assert_eq!(format_search_count("zzz", p(-1, 0)).as_deref(), Some("无结果"), "count=0 排在 index<0 前面");
        assert_eq!(format_search_count("a", p(-1, 1000)).as_deref(), Some("1000 个结果"), "超出高亮上限只报总数");
        assert_eq!(format_search_count("", p(-1, 0)), None);
        assert_eq!(format_search_count("  ", p(-1, 0)), None);
        assert_eq!(format_search_count("foo", None), None);
        assert_eq!(format_search_count("foo", p(17, 17)).as_deref(), Some("17 个结果"), "越界退回只报总数");
    }
}
