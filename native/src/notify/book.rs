//! 通知记录 reducer（纯逻辑，#215 第 4 节）。
//!
//! - **每个会话最多一条**，新的替换旧的（照 对标产品「同一 pane 只留最新」）；同一件事没处理之前不重复弹
//! - **名字不存**：渲染时按 session_id 从会话列表现取（#8：改名后通知里的名字跟着变）
//! - 已读 / 撤回 / 清除都返回「要撤回哪些系统横幅」，由调用方交给系统通知层
//! - 持久化只存记录；启动时恢复，但**不补发横幅**（`from_saved` 不产生任何 Effects）
//!
//! 为什么单独测：错了在 UI 上就是「铃铛红点挂着过期的未读」（#215 问题 6）、
//! 「横幅弹了通知中心却没加 / 时间不更新」（问题 3）、「重启一次弹一串」（问题 2）、
//! 「已读之后系统通知中心里那条还在」、「升级到 GPUI 版后 Tauri 版的通知记录丢了」。

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::deliver::Delivery;
use super::model::Kind;

pub const MAX_RECORDS: usize = 100;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Record {
    pub session_id: String,
    pub kind: Kind,
    /// Claude 的原话（hook 来的才有；状态文件那路为空）
    #[serde(default)]
    pub message: String,
    /// 毫秒时间戳
    pub at: i64,
    pub read: bool,
}

/// 要交给系统通知层的横幅（identifier = 会话 id，同一会话新的自动替换旧的）
#[derive(Clone, Debug, PartialEq)]
pub struct Banner {
    pub session_id: String,
    pub kind: Kind,
    pub message: String,
    pub sound: bool,
}

/// 一次操作的副作用
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Effects {
    /// 记录变了（要存盘、重画、更新 Dock 角标）
    pub changed: bool,
    pub banner: Option<Banner>,
    /// 要撤回的系统横幅（会话 id）
    pub withdraw: Vec<String>,
    /// 窗口闪一下
    pub flash: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Book {
    /// 最新在前
    pub records: Vec<Record>,
    /// 这一轮等待里已经闪过窗口的会话（不持久化）。离开等待 / Clear 时清掉，
    /// 下次进入等待再闪。**不占记录**：瞥一眼没处理、切走以后照样能正常通知（Tauri 版 flashedRef 的教训）
    flashed: HashSet<String>,
}

impl Book {
    /// 从 `NativeState.notifications` 恢复。认新形状（`Record`），也认 Tauri 版 localStorage 导入过来的旧形状
    /// （`{sessionId, timestamp, isRead, waitingFor, …}`）；同一会话多条只留最新的；坏条目跳过
    pub fn from_saved(values: &[Value]) -> Self {
        let mut all: Vec<Record> = values.iter().filter_map(parse_saved).collect();
        // 新的在前；同一会话只留最新一条
        all.sort_by(|a, b| b.at.cmp(&a.at));
        let mut seen = HashSet::new();
        all.retain(|r| seen.insert(r.session_id.clone()));
        all.truncate(MAX_RECORDS);
        Self { records: all, flashed: HashSet::new() }
    }

    pub fn to_saved(&self) -> Vec<Value> {
        self.records.iter().filter_map(|r| serde_json::to_value(r).ok()).collect()
    }

    pub fn unread_count(&self) -> usize {
        self.records.iter().filter(|r| !r.read).count()
    }

    pub fn get(&self, session_id: &str) -> Option<&Record> {
        self.records.iter().find(|r| r.session_id == session_id)
    }

    /// 来了一条通知（`d` 是 `deliver` 的结果）
    pub fn notify(&mut self, session_id: &str, kind: Kind, message: &str, at: i64, d: Delivery) -> Effects {
        let mut e = Effects::default();
        if d.flash && self.flashed.insert(session_id.to_string()) {
            e.flash = true;
        }
        if !d.record {
            return e;
        }
        let idx = self.records.iter().position(|r| r.session_id == session_id);
        if let Some(i) = idx {
            let r = &mut self.records[i];
            // 同一件事还没处理（未读、同类）：不重复弹，只把正文补上
            if !r.read && r.kind == kind {
                if !message.is_empty() && r.message != message {
                    r.message = message.to_string();
                    e.changed = true;
                }
                return e;
            }
            self.records.remove(i);
        }
        self.records.insert(0, Record { session_id: session_id.into(), kind, message: message.into(), at, read: !d.unread });
        self.records.truncate(MAX_RECORDS);
        e.changed = true;
        if d.banner {
            e.banner = Some(Banner { session_id: session_id.into(), kind, message: message.into(), sound: d.sound });
        } else if idx.is_some() {
            // 旧横幅可能还在系统通知中心里，而新这条不弹 —— 撤掉，免得两边说的不是一件事
            e.withdraw.push(session_id.into());
        }
        e
    }

    /// 离开等待：需要处理的那条标已读 + 撤回；已完成 / 出错不动
    pub fn resolve(&mut self, session_id: &str) -> Effects {
        self.flashed.remove(session_id);
        match self.get(session_id) {
            Some(r) if r.kind.needs_action() => self.mark_read(session_id),
            _ => Effects::default(),
        }
    }

    /// 用户又发了一句 / 会话结束：全部标已读 + 撤回
    pub fn clear_session(&mut self, session_id: &str) -> Effects {
        self.flashed.remove(session_id);
        self.mark_read(session_id)
    }

    /// 看到了（切到这个标签、点横幅、点通知中心那条）
    pub fn mark_read(&mut self, session_id: &str) -> Effects {
        let mut e = Effects::default();
        if let Some(r) = self.records.iter_mut().find(|r| r.session_id == session_id && !r.read) {
            r.read = true;
            e.changed = true;
            e.withdraw.push(session_id.into());
        }
        e
    }

    pub fn mark_all_read(&mut self) -> Effects {
        let mut e = Effects::default();
        for r in self.records.iter_mut().filter(|r| !r.read) {
            r.read = true;
            e.changed = true;
            e.withdraw.push(r.session_id.clone());
        }
        e
    }

    /// 通知中心那条的 ×
    pub fn remove(&mut self, session_id: &str) -> Effects {
        let mut e = Effects::default();
        if let Some(i) = self.records.iter().position(|r| r.session_id == session_id) {
            self.records.remove(i);
            e.changed = true;
            e.withdraw.push(session_id.into());
        }
        e
    }

    pub fn clear_all(&mut self) -> Effects {
        let mut e = Effects::default();
        if !self.records.is_empty() {
            e.changed = true;
            e.withdraw = self.records.drain(..).map(|r| r.session_id).collect();
        }
        e
    }
}

/// 一条存档 → Record。新形状直接反序列化；否则按 Tauri 版旧形状读
fn parse_saved(v: &Value) -> Option<Record> {
    if let Ok(r) = serde_json::from_value::<Record>(v.clone()) {
        return Some(r);
    }
    let sid = v.get("sessionId")?.as_str().filter(|s| !s.is_empty())?;
    let at = v.get("timestamp")?.as_i64()?;
    let read = v.get("isRead").and_then(Value::as_bool).unwrap_or(true);
    let kind = if v.get("waitingFor").and_then(Value::as_str) == Some("user") { Kind::NeedsInput } else { Kind::NeedsPermission };
    Some(Record { session_id: sid.into(), kind, message: String::new(), at, read })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const LOUD: Delivery = Delivery { record: true, unread: true, banner: true, sound: true, flash: false };
    const QUIET: Delivery = Delivery { record: true, unread: true, banner: false, sound: false, flash: false };
    const FLASH: Delivery = Delivery { record: false, unread: false, banner: false, sound: false, flash: true };

    fn banner(sid: &str, kind: Kind, msg: &str, sound: bool) -> Option<Banner> {
        Some(Banner { session_id: sid.into(), kind, message: msg.into(), sound })
    }

    #[test]
    fn one_record_per_session_newest_first() {
        let mut b = Book::default();
        let e = b.notify("a", Kind::NeedsPermission, "用 Bash？", 1000, LOUD);
        assert!(e.changed);
        assert_eq!(e.banner, banner("a", Kind::NeedsPermission, "用 Bash？", true));
        b.notify("b", Kind::NeedsInput, "", 2000, LOUD);
        assert_eq!(b.records.iter().map(|r| r.session_id.as_str()).collect::<Vec<_>>(), ["b", "a"]);
        assert_eq!(b.unread_count(), 2);

        // 同一会话换了一类：替换旧的、挪到最前、再弹一次（identifier 相同，系统里旧横幅被替换）
        let e = b.notify("a", Kind::TurnComplete, "好了", 3000, LOUD);
        assert_eq!(b.records.len(), 2);
        assert_eq!(b.records[0], Record { session_id: "a".into(), kind: Kind::TurnComplete, message: "好了".into(), at: 3000, read: false });
        assert!(e.banner.is_some());
    }

    #[test]
    fn same_unread_event_does_not_ring_twice() {
        // #215 问题 3 + 清单 G「同一 session 有未读时不重复添加」：hook 和状态文件各报一次同一件事
        let mut b = Book::default();
        b.notify("a", Kind::NeedsPermission, "", 1000, LOUD);
        let e = b.notify("a", Kind::NeedsPermission, "Claude needs your permission to use Bash", 1500, LOUD);
        assert_eq!(e.banner, None, "不再弹");
        assert!(e.changed, "但补上了正文");
        assert_eq!(b.records[0].message, "Claude needs your permission to use Bash");
        assert_eq!(b.records[0].at, 1000, "时间还是第一次的");
        let e = b.notify("a", Kind::NeedsPermission, "", 1600, LOUD);
        assert_eq!(e, Effects::default(), "空正文不覆盖已有正文，什么都没变");
        assert_eq!(b.records.len(), 1);
    }

    #[test]
    fn read_record_is_replaced_by_a_new_event() {
        // 清单 G「重启后已读的 session 允许再次触发通知」：已读的不挡新的
        let mut b = Book::default();
        b.notify("a", Kind::NeedsPermission, "", 1000, LOUD);
        b.mark_read("a");
        let e = b.notify("a", Kind::NeedsPermission, "", 2000, LOUD);
        assert!(e.banner.is_some());
        assert!(!b.records[0].read);
        assert_eq!(b.records[0].at, 2000);
    }

    #[test]
    fn quiet_delivery_records_without_banner_and_withdraws_stale_one() {
        let mut b = Book::default();
        b.notify("a", Kind::NeedsPermission, "", 1000, LOUD);
        // 旧横幅还在系统通知中心，新的一条不弹横幅（比如总开关关了）→ 撤回旧横幅，免得两边对不上
        let e = b.notify("a", Kind::TurnComplete, "", 2000, QUIET);
        assert_eq!(e.banner, None);
        assert_eq!(e.withdraw, ["a"]);
        assert_eq!(b.records[0].kind, Kind::TurnComplete);
    }

    #[test]
    fn flash_does_not_take_a_record_and_happens_once_per_wait() {
        let mut b = Book::default();
        let e = b.notify("a", Kind::NeedsPermission, "", 1000, FLASH);
        assert!(e.flash && !e.changed && e.banner.is_none());
        assert!(b.records.is_empty(), "正看着：不进中心");
        assert!(!b.notify("a", Kind::NeedsPermission, "", 1100, FLASH).flash, "同一轮等待只闪一次");
        // 切走了、它还在等：照样正常通知（Tauri 版 flashedRef 修的那个单向门）
        let e = b.notify("a", Kind::NeedsPermission, "", 1200, LOUD);
        assert!(e.banner.is_some());
        // 离开等待后，下一轮等待重新能闪
        b.resolve("a");
        assert!(b.notify("a", Kind::NeedsPermission, "", 1300, FLASH).flash);
        b.clear_session("a");
        assert!(b.notify("a", Kind::NeedsPermission, "", 1400, FLASH).flash);
    }

    #[test]
    fn resolve_only_touches_needs_action() {
        let mut b = Book::default();
        b.notify("a", Kind::NeedsPermission, "", 1000, LOUD);
        b.notify("c", Kind::TurnComplete, "好了", 1000, LOUD);
        let e = b.resolve("a");
        assert_eq!(e.withdraw, ["a"], "批准了：撤回横幅");
        assert!(b.get("a").unwrap().read, "#215 问题 6：在终端里批准后不再挂着未读");
        let e = b.resolve("c");
        assert_eq!(e, Effects::default(), "已完成不因离开等待而消失");
        assert!(!b.get("c").unwrap().read);
        assert_eq!(b.resolve("nope"), Effects::default());
    }

    #[test]
    fn clear_session_reads_everything_for_that_session() {
        let mut b = Book::default();
        b.notify("c", Kind::TurnComplete, "好了", 1000, LOUD);
        let e = b.clear_session("c");
        assert!(e.changed);
        assert_eq!(e.withdraw, ["c"]);
        assert!(b.get("c").unwrap().read);
        assert_eq!(b.clear_session("c").withdraw, Vec::<String>::new(), "已读的不重复撤回");
    }

    #[test]
    fn read_remove_and_clear_withdraw_banners() {
        let mut b = Book::default();
        b.notify("a", Kind::NeedsPermission, "", 1000, LOUD);
        b.notify("b", Kind::NeedsInput, "", 2000, LOUD);
        b.notify("c", Kind::Error, "e", 3000, LOUD);
        assert_eq!(b.mark_read("a").withdraw, ["a"]);
        assert_eq!(b.mark_read("a"), Effects::default(), "已读再标没变化");
        assert_eq!(b.unread_count(), 2);
        let e = b.mark_all_read();
        assert!(e.changed);
        let mut w = e.withdraw.clone();
        w.sort();
        assert_eq!(w, ["b", "c"], "只撤回原来未读的");
        assert_eq!(b.unread_count(), 0);

        let e = b.remove("b");
        assert!(e.changed);
        assert_eq!(e.withdraw, ["b"], "移除时系统通知中心里的也删掉");
        assert!(b.get("b").is_none());
        let e = b.clear_all();
        let mut w = e.withdraw.clone();
        w.sort();
        assert_eq!(w, ["a", "c"]);
        assert!(b.records.is_empty());
        assert_eq!(b.clear_all(), Effects::default());
    }

    #[test]
    fn capped_at_100() {
        let mut b = Book::default();
        for i in 0..130 {
            b.notify(&format!("s{i}"), Kind::NeedsPermission, "", i, QUIET);
        }
        assert_eq!(b.records.len(), MAX_RECORDS);
        assert_eq!(b.records[0].session_id, "s129");
        assert_eq!(b.records.last().unwrap().session_id, "s30", "挤掉最旧的");
    }

    #[test]
    fn restore_roundtrip_and_tauri_shape() {
        let mut b = Book::default();
        b.notify("a", Kind::NeedsPermission, "m", 1000, LOUD);
        b.notify("b", Kind::TurnComplete, "", 2000, QUIET);
        b.mark_read("a");
        let saved = b.to_saved();
        let back = Book::from_saved(&saved);
        assert_eq!(back.records, b.records, "往返");
        assert_eq!(back.unread_count(), 1);

        // Tauri 版 localStorage 里的旧形状：每次等待一条、带名字快照；同一会话可能多条
        let old = vec![
            json!({"id":"a-3","sessionId":"a","sessionName":"旧名字","projectLabel":"p","timestamp":3000,"isRead":false,"kind":"waiting","source":"hook"}),
            json!({"id":"b-2","sessionId":"b","sessionName":"x","projectLabel":"","timestamp":2000,"isRead":true,"kind":"waiting","waitingFor":"user"}),
            json!({"id":"a-1","sessionId":"a","sessionName":"旧名字","projectLabel":"p","timestamp":1000,"isRead":true,"kind":"waiting"}),
            json!({"坏的": true}),
            json!("也是坏的"),
        ];
        let b = Book::from_saved(&old);
        assert_eq!(
            b.records,
            vec![
                Record { session_id: "a".into(), kind: Kind::NeedsPermission, message: String::new(), at: 3000, read: false },
                Record { session_id: "b".into(), kind: Kind::NeedsInput, message: String::new(), at: 2000, read: true },
            ],
            "每会话留最新一条；waitingFor=user 是等回答；名字不带过来"
        );
    }

    #[test]
    fn restore_sorts_newest_first_and_caps() {
        let vals: Vec<Value> = (0..120)
            .map(|i| serde_json::to_value(Record { session_id: format!("s{i}"), kind: Kind::Error, message: String::new(), at: i, read: true }).unwrap())
            .collect();
        let b = Book::from_saved(&vals);
        assert_eq!(b.records.len(), MAX_RECORDS);
        assert_eq!(b.records[0].at, 119);
    }
}
