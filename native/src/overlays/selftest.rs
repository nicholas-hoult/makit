//! 浮层的无人值守自检：`MAKIT_NATIVE_SELFTEST=overlays`（配合假 HOME 和 `MAKIT_NATIVE_FORCE_DRAW=1`）。
//!
//! 按真实键位（走整张快捷键表）开关每个浮层，每步打印宿主里开着哪些浮层；渲染代码每帧都跑，
//! 有 panic 当场暴露。看不到画面时用它验证「开得出来、Esc 关得掉、焦点回得去」。

use std::time::Duration;

use gpui::{point, px, App, AppContext, AsyncApp, Entity, Keystroke, WindowHandle};

use super::{host, MenuItem};
use crate::app::Root;
use crate::state::AppState;

fn opened(cx: &mut AsyncApp) -> String {
    cx.update(|cx| host(cx).map(|h| h.read(cx).debug_open()).unwrap_or_default()).unwrap_or_default()
}

/// 详情面板的阅读视图（#231 第 2 期）：读到 Item、折叠展开、会话文件追加一行后 ≤ 300ms（这里等 900ms）出现。
/// 追加会改会话文件，只在 `MAKIT_SELFTEST_APPEND=1`（假 HOME 里的合成会话）时做
async fn detail_check(
    cx: &mut AsyncApp,
    sid: &str,
    pause: &impl Fn(u64) -> gpui::Task<()>,
    fails: &std::rc::Rc<std::cell::RefCell<Vec<String>>>,
) {
    let fail = |m: String| {
        eprintln!("[selftest] ✗ 阅读视图：{m}");
        fails.borrow_mut().push(format!("阅读视图：{m}"));
    };
    let view = cx.update(|cx| host(cx).and_then(|h| h.read(cx).detail.as_ref().map(|d| d.view.clone()))).ok().flatten();
    let Some(view) = view else { return fail("详情面板没开".into()) };
    let state = |cx: &mut AsyncApp| cx.update(|cx| view.read(cx).debug_state()).unwrap();
    let (n0, _) = state(cx);
    eprintln!("[selftest] 阅读视图：读到 {n0} 项");
    if n0 == 0 {
        return fail("一项都没读到".into());
    }
    // MAKIT_SELFTEST_SHOT=<前缀>：折叠 / 展开各截一张屏（要窗口可见、屏幕没锁）
    let shot = |name: &str| {
        if let Ok(prefix) = std::env::var("MAKIT_SELFTEST_SHOT") {
            // 自检窗口不一定在前台：先按 pid 把自己提到最前，不然截到的是别的窗口
            let script = format!("tell application \"System Events\" to set frontmost of (first process whose unix id is {}) to true", std::process::id());
            let _ = std::process::Command::new("osascript").args(["-e", &script]).status();
            std::thread::sleep(Duration::from_millis(600));
            let _ = std::process::Command::new("screencapture").args(["-x", &format!("{prefix}-{name}.png")]).status();
        }
    };
    pause(500).await;
    shot("folded");
    if cx.update(|cx| view.update(cx, |v, cx| v.debug_toggle_first_tool(cx))).unwrap() {
        pause(500).await;
        shot("expanded");
        if state(cx).1 != 1 {
            fail("点开工具调用后展开数不是 1".into());
        }
        cx.update(|cx| view.update(cx, |v, cx| v.debug_toggle_first_tool(cx))).unwrap();
        pause(200).await;
        if state(cx).1 != 0 {
            fail("再点一次没有收起".into());
        }
    }
    if std::env::var("MAKIT_SELFTEST_APPEND").is_ok() {
        let Some((path, _)) = makit_core::sessions::locate_session_file(sid) else { return fail("找不到会话文件".into()) };
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let parent = text
            .lines()
            .rev()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .find(|v| matches!(v["type"].as_str(), Some("user" | "assistant")))
            .and_then(|v| v["uuid"].as_str().map(String::from));
        let line = serde_json::json!({"type":"user","uuid":format!("selftest-appended-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)),"parentUuid":parent,"timestamp":"2026-09-30T00:00:00Z","sessionId":sid,
            "message":{"role":"user","content":"实时追加的一行"}});
        use std::io::Write;
        let _ = writeln!(std::fs::OpenOptions::new().append(true).open(&path).unwrap(), "{line}");
        pause(900).await;
        let (n1, _) = state(cx);
        if n1 != n0 + 1 {
            fail(format!("追加一行后 Item 数应为 {}，实际 {n1}", n0 + 1));
        } else {
            eprintln!("[selftest] 阅读视图：追加后 {n0} → {n1}");
        }
    }
}

/// 命令面板里的滚动条（scrollbar.rs）：显形 / 淡出时序 + 真实鼠标拖动
async fn scrollbar_check(
    cx: &mut AsyncApp,
    any: gpui::AnyWindowHandle,
    pause: &impl Fn(u64) -> gpui::Task<()>,
    fails: &std::rc::Rc<std::cell::RefCell<Vec<String>>>,
) {
    use crate::sidebar::tree::{scroll_for_thumb, thumb_geometry};
    let fail = |m: String| {
        eprintln!("[selftest] ✗ 滚动条：{m}");
        fails.borrow_mut().push(format!("滚动条：{m}"));
    };
    let got = cx
        .update(|cx| host(cx).and_then(|h| h.read(cx).palette.as_ref().map(|p| p.view.read(cx).debug_scroll())))
        .ok()
        .flatten();
    let Some((scroll, bar)) = got else { return fail("命令面板没开".into()) };
    let m = cx.update(|cx| bar.read(cx).debug_metrics()).unwrap();
    eprintln!("[selftest] 滚动条：列表 total={:.0} viewport={:.0}（bounds {:?} max_offset {:?}）", m.total, m.viewport, scroll.bounds(), scroll.max_offset());
    if m.total <= m.viewport {
        return fail("命令面板的列表没有溢出（要用 250 个会话的假 HOME 跑）".into());
    }

    // 1. 显形 / 淡出：程序滚一下（和滚轮同一个状态），条子亮，900ms 后灭
    let state_of = |cx: &mut AsyncApp| cx.update(|cx| bar.read(cx).debug_state()).unwrap();
    if state_of(cx).0 {
        return fail("还没滚就亮着".into());
    }
    scroll.set_offset(point(px(0.0), px(-200.0)));
    let _ = cx.update_window(any, |_, window, _| window.refresh());
    pause(250).await;
    if !state_of(cx).0 {
        fail("滚动之后没有显形".into());
    }
    pause(1200).await;
    if state_of(cx).0 {
        fail("停手 1.2s 之后还亮着（900ms 该淡出）".into());
    }

    // 2. 拖动换算：按在滑块中间，往下拖 40px（gpui 没有对外的鼠标事件注入，直接走 debug_drag；
    //    on_drag / on_drag_move 的事件投递和侧栏滚动条是同一套，那边已经在用）
    scroll.set_offset(point(px(0.0), px(0.0)));
    let _ = cx.update_window(any, |_, window, _| window.refresh());
    pause(300).await;
    let m = cx.update(|cx| bar.read(cx).debug_metrics()).unwrap();
    let (thumb_top, thumb_h) = thumb_geometry(m.total, m.viewport, m.top).unwrap();
    let track_top = f32::from(scroll.bounds().top());
    let grab = thumb_h / 2.0;
    let pointer_y = track_top + thumb_top + grab + 40.0;
    let _ = cx.update(|cx| bar.update(cx, |b, cx| b.debug_drag(grab, pointer_y, track_top, cx)));
    let (_, dragging) = state_of(cx);
    if !dragging {
        fail("拖动之后滚动条不在拖动状态".into());
    }
    let after = cx.update(|cx| bar.read(cx).debug_metrics()).unwrap();
    let want = scroll_for_thumb(m.total, m.viewport, thumb_top + 40.0);
    eprintln!("[selftest] 滚动条：拖 40px 之后滚动量 {:.1}，期望 {:.1}（滑块 {:.0}px 高）", after.top, want, thumb_h);
    if (after.top - want).abs() > 1.0 || after.top < 1.0 {
        fail(format!("拖动后滚动量 {:.1}，期望 {:.1}", after.top, want));
    }
    // 松手（这里没有真的拖拽，渲染会把 dragging 复位）
    pause(200).await;
    if state_of(cx).1 {
        fail("没有活动拖拽时 dragging 应该复位".into());
    }
    eprintln!("[selftest] ✓ 滚动条：显形 / 淡出 / 拖动检查完毕");
}

pub fn run(handle: WindowHandle<Root>, state: Entity<AppState>, cx: &mut App) {
    cx.spawn(async move |cx| {
        let ex = cx.background_executor().clone();
        let pause = move |ms| ex.timer(Duration::from_millis(ms));
        for _ in 0..600 {
            pause(100).await;
            if cx.read_entity(&state, |s, _| s.loaded).unwrap_or(false) {
                break;
            }
        }
        let any: gpui::AnyWindowHandle = handle.into();
        let press = |cx: &mut AsyncApp, keys: &str| {
            let ks = Keystroke::parse(keys).expect("键位写错了");
            let _ = cx.update_window(any, |_, window, cx| window.dispatch_keystroke(ks, cx));
        };
        let fails = std::rc::Rc::new(std::cell::RefCell::new(Vec::<String>::new()));
        let expect = {
            let fails = fails.clone();
            move |cx: &mut AsyncApp, step: &str, want: &str| {
                let got = opened(cx);
                eprintln!("[selftest] {step}：开着 [{got}]");
                if got != want {
                    fails.borrow_mut().push(format!("{step}：期望 [{want}]，实际 [{got}]"));
                }
            }
        };

        // ⌘K：打开、输入、上下、Esc 关
        press(cx, "cmd-k");
        pause(300).await;
        expect(cx, "⌘K 打开", "palette");
        for k in ["x", "y", "z", "backspace"] {
            press(cx, k);
            pause(80).await;
        }
        let q = cx.update(|cx| host(cx).and_then(|h| h.read(cx).palette.as_ref().map(|p| p.view.read(cx).query().to_string()))).ok().flatten();
        eprintln!("[selftest] ⌘K 输入 xyz⌫ 之后 query = {q:?}");
        if q.as_deref() != Some("xy") {
            fails.borrow_mut().push(format!("⌘K 输入框：期望 \"xy\"，实际 {q:?}"));
        }
        for k in ["backspace", "backspace", "down", "down", "up"] {
            press(cx, k);
            pause(80).await;
        }
        press(cx, "cmd-k");
        pause(300).await;
        expect(cx, "⌘K 再按一次关", "");
        press(cx, "cmd-k");
        pause(300).await;
        press(cx, "escape");
        pause(300).await;
        expect(cx, "⌘K 里 Esc 关", "");

        // 滚动条：命令面板的列表溢出时，滚动 → 显形 → 900ms 后淡出；真实鼠标事件按住滑块拖动，内容跟着走
        press(cx, "cmd-k");
        pause(300).await;
        // 无搜索词时每组只列 10 条，放得下；打个字母让结果多起来（搜索时每组最多 30 条）
        press(cx, "e");
        pause(500).await;
        scrollbar_check(cx, any, &pause, &fails).await;
        press(cx, "escape");
        pause(300).await;
        expect(cx, "滚动条检查后 Esc 关面板", "");

        // ⌘, 设置
        press(cx, "cmd-,");
        pause(300).await;
        expect(cx, "⌘, 打开设置", "settings");
        press(cx, "escape");
        pause(300).await;
        expect(cx, "设置里 Esc 关（#224）", "");

        // ⌘F 搜索条
        press(cx, "cmd-f");
        pause(300).await;
        for k in ["a", "enter", "shift-enter"] {
            press(cx, k);
            pause(80).await;
        }
        expect(cx, "⌘F 打开搜索条", "search");
        press(cx, "escape");
        pause(300).await;
        expect(cx, "搜索条 Esc 关", "");

        // 右键菜单：贴右下角弹，Esc 关
        let _ = cx.update_window(any, |_, window, cx| {
            let vp = window.viewport_size();
            super::show_context_menu(point(vp.width - px(2.0), vp.height - px(2.0)), window, cx, |_| {
                vec![MenuItem::header("组"), MenuItem::action("一", |_, _| {}).checked(true), MenuItem::separator(), MenuItem::action("二", |_, _| {}).disabled(true)]
            });
        });
        pause(300).await;
        expect(cx, "右键菜单打开", "menu");
        press(cx, "escape");
        pause(300).await;
        expect(cx, "右键菜单 Esc 关", "");

        // toast
        let _ = cx.update(|cx| super::show_toast("自检 toast", 400, cx));
        pause(100).await;
        expect(cx, "toast 出现", "toast");
        pause(700).await;
        expect(cx, "toast 到时消失", "");

        // 详情面板（有会话才测）
        let first = cx.read_entity(&state, |s, _| s.sessions.first().map(|m| m.session_id.clone())).ok().flatten();
        if let Some(sid) = first {
            let _ = cx.update_window(any, |_, window, cx| super::open_detail(&sid, window, cx));
            pause(1500).await;
            expect(cx, "详情面板打开", "detail");
            detail_check(cx, &sid, &pause, &fails).await;
            press(cx, "escape");
            pause(300).await;
            expect(cx, "详情 Esc 关（#224）", "");
        } else {
            eprintln!("[selftest] 没有会话，跳过详情面板");
        }

        // 恢复 cwd 选择器（#239）：新建一个标签，拿它报一个不存在的目录；选择器挂在 pane 底部，不占浮层槽位
        press(cx, "cmd-t");
        pause(800).await;
        let tab = cx.read_entity(&state, |s, _| s.workspace.active_tab().map(|t| t.id.clone())).ok().flatten();
        if let Some(tab) = tab {
            let _ = cx.update_window(any, |_, window, cx| super::recover::cwd_missing(&tab, "/nonexistent/makit-selftest", window, cx));
            pause(500).await;
            let dbg = |cx: &mut AsyncApp| cx.update(|cx| host(cx).and_then(|h| h.read(cx).workspace.read(cx).debug_recover(&tab, cx))).ok().flatten();
            match dbg(cx) {
                Some((n, sel, _)) => {
                    eprintln!("[selftest] 恢复选择器打开：{n} 项，选中第 {} 项", sel + 1);
                    if n < 2 {
                        fails.borrow_mut().push(format!("恢复选择器：选项数 {n} < 2（至少有重建 / 选择目录）"));
                    }
                    press(cx, "down");
                    pause(150).await;
                    let after = dbg(cx).map(|d| d.1);
                    if after != Some(1 % n) {
                        fails.borrow_mut().push(format!("恢复选择器：↓ 之后选中 {after:?}，期望 {}", 1 % n));
                    }
                    press(cx, "up");
                    pause(150).await;
                    if dbg(cx).map(|d| d.1) != Some(0) {
                        fails.borrow_mut().push("恢复选择器：↑ 之后没回到第一项".into());
                    }
                    press(cx, "escape");
                    pause(300).await;
                    if dbg(cx).is_some() {
                        fails.borrow_mut().push("恢复选择器：Esc 之后还在".into());
                    } else {
                        eprintln!("[selftest] 恢复选择器 Esc 关");
                    }
                }
                None => fails.borrow_mut().push("恢复选择器没出来".into()),
            }
        }

        let fails = fails.borrow();
        if fails.is_empty() {
            eprintln!("[selftest] 通过");
        } else {
            eprintln!("[selftest] 失败：{}", fails.join("；"));
        }
        let _ = cx.update(|cx| cx.quit());
    })
    .detach();
}
