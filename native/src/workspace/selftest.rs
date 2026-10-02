//! 工作区自检（`MAKIT_NATIVE_SELFTEST=workspace`）：锁屏看不到画面时验证 C 包的界面逻辑 + 渲染代码不 panic。
//!
//! 按真实键位（走整张快捷键表）驱动：欢迎卡 → pane 右键菜单 → ⌘T / ⌘D → ⌥⌘1 / ⌥⌘→ 闪牌（图标按 pane 序号）→
//! 拖分割线（改 ratio）→ 标签右键菜单 → ⌥⌘↩ 最大化 → ⌘W。每一步之后等几帧（配合 `MAKIT_NATIVE_FORCE_DRAW=1`
//! 逼 GPUI 画），渲染路径有 panic 会直接崩。不恢复任何会话（不碰用户真实的 claude 会话）。

use std::time::Duration;

use gpui::{App, AppContext, Entity, WindowHandle};

use super::labels::titlebar_text;
use super::model::collect_containers;
use super::{MenuTarget, WorkspaceView};
use crate::app::Root;
use crate::state::AppState;

pub fn run(handle: WindowHandle<Root>, state: Entity<AppState>, ws: Entity<WorkspaceView>, cx: &mut App) {
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
        // 真实键位派发（走整张快捷键表 + 焦点链），和用户按键同一条路
        let press = |cx: &mut gpui::AsyncApp, keys: &str| {
            let ks = gpui::Keystroke::parse(keys).expect("键位写错了");
            let _ = cx.update_window(any, |_, window, cx| window.dispatch_keystroke(ks, cx));
        };
        let mut fails: Vec<String> = Vec::new();
        let mut check = |ok: bool, what: &str| {
            eprintln!("[selftest] {} {what}", if ok { "✓" } else { "✗" });
            if !ok {
                fails.push(what.to_string());
            }
        };

        // 1. 从空工作区开始（欢迎卡）
        let _ = state.update(cx, |s, cx| {
            s.workspace = super::model::Workspace::default();
            s.workspace_changed(cx);
        });
        pause(500).await;
        let title = cx.read_entity(&state, |s, _| titlebar_text(s.workspace.active_tab(), None)).unwrap_or_default();
        check(title == "makit", &format!("没有标签时标题栏是 makit（实际 {title}）"));

        // 2. pane 右键菜单（欢迎卡上）
        let cid = cx.read_entity(&state, |s, _| s.workspace.state.active_container_id.clone()).unwrap_or_default();
        let n = cx.update(|cx| ws.update(cx, |v, cx| v.menu_items(&MenuTarget::Pane { cid: cid.clone() }, cx).len())).unwrap_or(0);
        check(n == 5, &format!("pane 菜单 5 项（左右 / 上下 / 新终端 / ─ / 关闭当前 tab），实际 {n}"));
        // 弹菜单 / 关菜单现在都是 D 包的通用组件（overlays::OverlayHost），不再是工作区自己的状态
        let _ = cx.update_window(any, |_, window, cx| {
            let weak = ws.downgrade();
            let target = MenuTarget::Pane { cid: cid.clone() };
            crate::overlays::show_context_menu(gpui::point(gpui::px(200.0), gpui::px(200.0)), window, cx, move |cx| {
                weak.upgrade().map(|w| w.read(cx).menu_items(&target, cx)).unwrap_or_default()
            });
        });
        pause(400).await;
        let is_open = cx.update(|cx| crate::overlays::host(cx).map(|h| h.read(cx).menu_open()).unwrap_or(false)).unwrap_or(false);
        check(is_open, "pane 右键菜单弹出后 OverlayHost.menu_open()");
        let _ = cx.update_window(any, |_, window, cx| {
            if let Some(h) = crate::overlays::host(cx) {
                h.update(cx, |h, cx| h.close_menu(window, cx));
            }
        });

        // 3. ⌘T ⌘D → 两个 pane
        press(cx, "cmd-t");
        pause(700).await;
        press(cx, "cmd-d");
        pause(700).await;
        let panes = cx.read_entity(&state, |s, _| collect_containers(&s.workspace.state.root).len()).unwrap_or(0);
        check(panes == 2, &format!("⌘T ⌘D 之后 2 个 pane（实际 {panes}）"));
        let title = cx.read_entity(&state, |s, _| titlebar_text(s.workspace.active_tab(), None)).unwrap_or_default();
        check(!title.is_empty() && title != "makit", &format!("有标签时标题栏是 cwd 的 basename（实际 {title}）"));

        // 4. ⌥⌘1 / ⌥⌘→ 闪牌
        // 帧耗时：先量 1s 空闲（没有动画）当对照，再量闪牌期间。强制绘制每 16ms 画一次，所以「帧数」≈ 时长 / 16ms
        pause(300).await;
        let _ = crate::selftest::take_frame_stats();
        pause(1000).await;
        let idle = crate::selftest::take_frame_stats();
        press(cx, "alt-cmd-1");
        pause(100).await;
        let f = cx.read_entity(&ws, |v, _| v.flash.as_ref().map(|f| (f.icon, f.name.clone()))).ok().flatten();
        check(matches!(&f, Some((Some("🐉"), _))), &format!("⌥⌘1 闪第 1 个 pane 的图标 🐉（实际 {f:?}）"));
        press(cx, "alt-cmd-right");
        pause(100).await;
        let f = cx.read_entity(&ws, |v, _| v.flash.as_ref().map(|f| f.icon)).ok().flatten();
        check(f == Some(Some("🐯")), &format!("⌥⌘→ 闪第 2 个 pane 的图标 🐯（实际 {f:?}）"));
        pause(900).await;
        let gone = cx.read_entity(&ws, |v, _| v.flash.is_none()).unwrap_or(false);
        check(gone, "闪牌 0.85s 后自己消失");
        let flash = crate::selftest::take_frame_stats();
        eprintln!("[selftest] 帧耗时 空闲：{} 帧 平均 {:.2}ms p95 {:.2}ms 最大 {:.2}ms", idle.0, idle.1, idle.2, idle.3);
        eprintln!("[selftest] 帧耗时 闪牌：{} 帧 平均 {:.2}ms p95 {:.2}ms 最大 {:.2}ms", flash.0, flash.1, flash.2, flash.3);
        // 闪牌动画不能明显拖慢每一帧：平均耗时不超过空闲时的 2 倍 + 1ms（实测 release 空闲 2.8ms / 闪牌 4.0ms；
        // debug 版空闲就要 15ms，所以用比值而不是绝对值）。光晕的模糊阴影、边框、图标缩放加起来出过 2 倍以上就该查
        check(flash.0 > 0 && idle.0 > 0 && flash.1 <= idle.1 * 2.0 + 1.0, &format!("闪牌期间每帧平均耗时 {:.2}ms 不超过空闲 {:.2}ms 的 2 倍 + 1ms", flash.1, idle.1));

        // 5. 拖分割线（直接改 ratio，模拟拖动中的状态）
        let sid = cx.read_entity(&state, |s, _| match &s.workspace.state.root {
            super::model::LayoutNode::Split(sp) => Some(super::model::split_id(sp)),
            _ => None,
        });
        if let Ok(Some(sid)) = sid {
            let _ = cx.update(|cx| {
                cx.set_global(super::PaneResizing(true));
                state.update(cx, |s, cx| {
                    s.workspace.set_split_ratio(&sid, 0.3);
                    s.workspace_changed(cx);
                })
            });
            pause(300).await;
            let _ = cx.update(|cx| ws.update(cx, |v, cx| {
                v.split_drag = Some(super::SplitDragStart { id: sid.clone(), dir: super::model::Dir::V, start_pos: 0.0, start_ratio: 0.3 });
                v.end_split_drag(cx);
            }));
            let r = cx.read_entity(&state, |s, _| match &s.workspace.state.root {
                super::model::LayoutNode::Split(sp) => sp.ratio,
                _ => 0.0,
            })
            .unwrap_or(0.0);
            let resizing = cx.update(|cx| cx.global::<super::PaneResizing>().0).unwrap_or(true);
            check(r == 0.3 && !resizing, &format!("分割线 ratio 改成 0.3、松手后 PaneResizing 复位（ratio {r:?}，resizing {resizing}）"));
        } else {
            check(false, "⌘D 之后根节点应该是 split");
        }

        // 6. 标签右键菜单
        let (cid, tid) = cx
            .read_entity(&state, |s, _| s.workspace.active_container().map(|c| (c.id.clone(), c.active_tab_id.clone())).unwrap_or_default())
            .unwrap_or_default();
        let n = cx.update(|cx| ws.update(cx, |v, cx| v.menu_items(&MenuTarget::Tab { cid: cid.clone(), tid: tid.clone() }, cx).len())).unwrap_or(0);
        check(n == 10, &format!("标签菜单 8 项 + 2 条分隔线（同 TS paneMenuItems，实际 {n}）"));
        let _ = cx.update_window(any, |_, window, cx| {
            let weak = ws.downgrade();
            let target = MenuTarget::Tab { cid, tid };
            crate::overlays::show_context_menu(gpui::point(gpui::px(300.0), gpui::px(60.0)), window, cx, move |cx| {
                weak.upgrade().map(|w| w.read(cx).menu_items(&target, cx)).unwrap_or_default()
            });
        });
        pause(400).await;
        let is_open = cx.update(|cx| crate::overlays::host(cx).map(|h| h.read(cx).menu_open()).unwrap_or(false)).unwrap_or(false);
        check(is_open, "标签右键菜单弹出后 OverlayHost.menu_open()");
        let _ = cx.update_window(any, |_, window, cx| {
            if let Some(h) = crate::overlays::host(cx) {
                h.update(cx, |h, cx| h.close_menu(window, cx));
            }
        });

        // 7. ⌥⌘↩ 最大化 / 还原，⌘W
        press(cx, "alt-cmd-enter");
        pause(500).await;
        let max = cx.read_entity(&state, |s, _| s.workspace.state.maximized_container_id.is_some()).unwrap_or(false);
        check(max, "⌥⌘↩ 最大化");
        press(cx, "alt-cmd-enter");
        pause(500).await;
        press(cx, "cmd-w");
        pause(700).await;
        let panes = cx.read_entity(&state, |s, _| collect_containers(&s.workspace.state.root).len()).unwrap_or(0);
        check(panes == 1, &format!("⌘W 关掉右边 pane 唯一的标签后剩 1 个 pane（实际 {panes}）"));

        let _ = cx.read_entity(&state, |s, _| s.flush());
        if fails.is_empty() {
            eprintln!("[selftest] 通过");
        } else {
            eprintln!("[selftest] 失败：{}", fails.join("；"));
        }
        let _ = cx.update(|cx| cx.quit());
    })
    .detach();
}
