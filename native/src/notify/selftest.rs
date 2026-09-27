//! `MAKIT_NATIVE_SELFTEST=notify`：通知链路无人值守自检（锁屏也能跑）。
//!
//! **必须用假 HOME 跑**（hook socket 在 `$HOME/.claude/makit/hook.sock`，真 HOME 下会抢走正在用的 makit 的 hook）：
//! ```sh
//! mkdir -p /tmp/mke/h && env -u ZDOTDIR HOME=/tmp/mke/h MAKIT_NATIVE_SELFTEST=notify MAKIT_NATIVE_FORCE_DRAW=1 target/debug/makit-native
//! ```
//! 真的往 hook socket 里写 Claude Code 的 hook JSON，走一遍：分类 → 去重 → 记录 → 未读数 / Dock 角标 →
//! ⌘I 开抽屉、↓ / Esc / Enter 键盘 → 跳转 → UserPromptSubmit 清除 → 正看着的会话只闪不记 → 存盘。
//! 裸二进制没有 app 身份，不会发系统横幅（日志里有「未发横幅」行）。

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use gpui::{AnyWindowHandle, App, AppContext, AsyncApp, Entity, Keystroke};

use crate::state::AppState;

use super::center::Notifier;
use super::model::Kind;

const A: &str = "selftest-aaaaaaaa-0000";
const B: &str = "selftest-bbbbbbbb-0000";
const C: &str = "selftest-cccccccc-0000";

fn send_hook(line: String) -> Result<String, String> {
    let path = makit_core::hook::socket_path().ok_or("没有 HOME")?;
    let mut s = UnixStream::connect(&path).map_err(|e| format!("连不上 {}：{e}", path.display()))?;
    s.write_all(line.as_bytes()).and_then(|_| s.write_all(b"\n")).map_err(|e| e.to_string())?;
    s.shutdown(std::net::Shutdown::Write).map_err(|e| e.to_string())?;
    let mut reply = String::new();
    s.read_to_string(&mut reply).map_err(|e| e.to_string())?;
    Ok(reply)
}

pub fn run(window: AnyWindowHandle, state: Entity<AppState>, cx: &mut App) {
    let home = std::env::var("HOME").unwrap_or_default();
    if !home.starts_with("/tmp/") && !home.starts_with("/private/tmp/") {
        eprintln!("[selftest] 失败：notify 自检必须用假 HOME（/tmp/...），现在是 {home} —— 真 HOME 下会抢走正在用的 makit 的 hook");
        cx.quit();
        return;
    }
    cx.spawn(async move |cx: &mut AsyncApp| {
        let ex = cx.background_executor().clone();
        let pause = |ms| ex.timer(Duration::from_millis(ms));
        for _ in 0..600 {
            pause(100).await;
            if cx.read_entity(&state, |s, _| s.loaded).unwrap_or(false) {
                break;
            }
        }
        pause(500).await;
        let Ok(n) = cx.update(|cx| Notifier::global(cx)) else { return };
        let mut fails: Vec<String> = Vec::new();
        let mut check = |ok: bool, what: &str| {
            eprintln!("[selftest] {} {what}", if ok { "✓" } else { "✗" });
            if !ok {
                fails.push(what.to_string());
            }
        };
        let hook = |ex: &gpui::BackgroundExecutor, v: serde_json::Value| {
            let line = v.to_string();
            ex.spawn(async move { send_hook(line) })
        };
        let press = |cx: &mut AsyncApp, keys: &str| {
            let ks = Keystroke::parse(keys).expect("键位写错了");
            let _ = cx.update_window(window, |_, w, cx| w.dispatch_keystroke(ks, cx));
        };
        // 打开抽屉 = 点铃铛。不用 ⌘I：F0 骨架空工作区时窗口里没有任何焦点，GPUI 不派发按键
        // （F0 自己的 layout 自检同样卡在这），⌘I 在抽屉有焦点时的「关」下面单独按键测
        let open = |cx: &mut AsyncApp| {
            let n = n.clone();
            let _ = cx.update_window(window, |_, w, cx| n.update(cx, |x, cx| x.open(w, cx)));
        };
        let snap = |cx: &mut AsyncApp| {
            cx.read_entity(&n, |x, _| (x.book.records.clone(), x.unread_count(), x.open, x.selected, x.flash_seq)).unwrap()
        };

        // 1. 等审批：hook 进来 → 一条未读
        let r = hook(&ex, serde_json::json!({"hook_event_name":"Notification","session_id":A,"notification_type":"permission_prompt","message":"Claude needs your permission to use Bash"})).await;
        check(r.as_deref() == Ok("{}\n"), &format!("hook 回 {{}}（{r:?}）"));
        pause(400).await;
        let (recs, unread, ..) = snap(cx);
        check(recs.len() == 1 && recs[0].kind == Kind::NeedsPermission && unread == 1, &format!("等审批进中心、未读 1（{} 条，未读 {unread}）", recs.len()));

        // 2. 同一件事再报一次：不重复
        let _ = hook(&ex, serde_json::json!({"hook_event_name":"Notification","session_id":A,"notification_type":"permission_prompt","message":"Claude needs your permission to use Bash"})).await;
        pause(300).await;
        let (recs, unread, ..) = snap(cx);
        check(recs.len() == 1 && unread == 1, "同一件事不重复记");

        // 3. 已完成（带原话）
        let _ = hook(&ex, serde_json::json!({"hook_event_name":"Stop","session_id":B,"stop_hook_active":false,"last_assistant_message":"改好了"})).await;
        // 还有后台任务的 Stop 不算完成
        let _ = hook(&ex, serde_json::json!({"hook_event_name":"Stop","session_id":C,"background_tasks":[{"id":"x"}]})).await;
        pause(400).await;
        let (recs, unread, ..) = snap(cx);
        check(recs.len() == 2 && recs[0].session_id == B && recs[0].kind == Kind::TurnComplete && unread == 2, "已完成进中心、最新在前；有后台任务的 Stop 不记");

        // 4. 开抽屉，默认高亮第一条未读；↓ 循环；Esc 关；⌘I 关
        open(cx);
        pause(300).await;
        let (_, _, is_open, sel, _) = snap(cx);
        check(is_open && sel == Some(0), &format!("打开、高亮第一条未读（open={is_open} sel={sel:?}）"));
        press(cx, "down");
        pause(150).await;
        check(snap(cx).3 == Some(1), "↓ 下一条");
        press(cx, "down");
        pause(150).await;
        check(snap(cx).3 == Some(0), "↓ 到底循环回第一条");
        press(cx, "up");
        pause(150).await;
        check(snap(cx).3 == Some(1), "↑ 循环到最后一条");
        press(cx, "escape");
        pause(200).await;
        check(!snap(cx).2, "Esc 关闭");
        open(cx);
        pause(300).await;
        press(cx, "cmd-i");
        pause(200).await;
        check(!snap(cx).2, "抽屉开着时 ⌘I 关闭");

        // 5. 再开，Enter：标已读 + 跳转（会话不在任何标签里 → 侧栏定位事件）+ 关
        open(cx);
        pause(300).await;
        press(cx, "enter");
        pause(300).await;
        let (recs, unread, is_open, ..) = snap(cx);
        check(!is_open && recs.iter().find(|r| r.session_id == B).is_some_and(|r| r.read) && unread == 1, &format!("Enter 已读并关闭（未读 {unread}）"));

        // 6. 用户在终端里接着发话（UserPromptSubmit）→ 清掉 A
        let _ = hook(&ex, serde_json::json!({"hook_event_name":"UserPromptSubmit","session_id":A,"prompt":"好"})).await;
        pause(300).await;
        check(snap(cx).1 == 0, "UserPromptSubmit 清除，未读 0");

        // 7. 正看着的会话进入等待：只闪窗口，不进中心
        // 用 shell 标签绑会话（不用 open_session：那会真的去跑 claude --resume）
        let _ = state.update(cx, |s, cx| {
            let tid = s.workspace.open_shell("/tmp");
            let cid = s.workspace.state.active_container_id.clone();
            s.workspace.bind_session_to_tab(&cid, &tid, C, "cccccccc", None, None);
            s.workspace_changed(cx);
        });
        let _ = n.update(cx, |x, cx| x.set_window_active(true, cx));
        pause(200).await;
        let flash0 = snap(cx).4;
        let _ = hook(&ex, serde_json::json!({"hook_event_name":"Notification","session_id":C,"notification_type":"permission_prompt","message":"m"})).await;
        pause(300).await;
        let (recs, _, _, _, flash1) = snap(cx);
        check(flash1 == flash0 + 1 && !recs.iter().any(|r| r.session_id == C), "正看着：闪窗口、不进中心");

        // 8. 全部清除 + 存盘
        let _ = n.update(cx, |x, cx| x.clear_all(cx));
        let _ = hook(&ex, serde_json::json!({"hook_event_name":"Notification","session_id":A,"notification_type":"idle_prompt","message":"Claude is waiting for your input"})).await;
        pause(300).await;
        let (recs, ..) = snap(cx);
        check(recs.is_empty(), "等回答默认关（prefs.notify.user=false）：不记");
        let _ = hook(&ex, serde_json::json!({"hook_event_name":"StopFailure","session_id":B,"error":"529 overloaded"})).await;
        pause(300).await;
        let _ = cx.read_entity(&state, |s, _| s.flush());
        let saved = std::fs::read_to_string(format!("{home}/.claude/makit/native-state.json")).unwrap_or_default();
        check(saved.contains("\"kind\": \"error\"") && saved.contains(B), "出错不受开关控制、记录写进 native-state.json");

        if fails.is_empty() {
            eprintln!("[selftest] 通过");
        } else {
            eprintln!("[selftest] 失败：{}", fails.join("；"));
        }
        let _ = cx.update(|cx| cx.quit());
    })
    .detach();
}
