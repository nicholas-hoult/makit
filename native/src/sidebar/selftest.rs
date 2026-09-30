//! 侧栏的无人值守自检：`MAKIT_NATIVE_SELFTEST=sidebar`（配 `MAKIT_NATIVE_FORCE_DRAW=1` 锁屏也能跑）。
//!
//! 用假 HOME 造的会话（见报告里的造数脚本）跑：按真实键位（整张快捷键表）走 ⌘⇧F、搜索框 ↓ 进列表、
//! ↑↓、←/→ 折叠、Enter 打开、⌘L 定位、Esc，切两种视图、展开 200 条的项目组，
//! 每步打印侧栏摘要；性能 = 改完数据到下一帧画完的耗时（含重算分组 + 布局 + 绘制）。
//! 输出以 `[selftest]` 开头，`[selftest] 通过` / `[selftest] 失败：…` 结尾。

use std::time::{Duration, Instant};

use gpui::{App, AppContext, AsyncApp, Entity, Keystroke, WindowHandle};

use crate::app::Root;
use crate::state::AppState;

use super::SidebarView;

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
        pause(500).await;
        let any: gpui::AnyWindowHandle = handle.into();
        let sidebar: Entity<SidebarView> = match handle.read_with(cx, |root, _| root.sidebar()) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[selftest] 失败：拿不到侧栏 {e}");
                let _ = cx.update(|cx| cx.quit());
                return;
            }
        };
        let mut failures: Vec<String> = Vec::new();
        let n = cx.read_entity(&state, |s, _| s.sessions.len()).unwrap_or(0);
        eprintln!("[selftest] 会话 {n} 条");

        let print = |cx: &mut AsyncApp, step: &str| -> usize {
            let (line, rows) = cx.read_entity(&sidebar, |v, cx| (v.debug_summary(cx), v.debug_rows())).unwrap_or_default();
            eprintln!("[selftest] {step}：{line}");
            rows
        };
        // 按键：先按真实键位走整张快捷键表；窗口不是 key window（锁屏 / 在后台）时 dispatch_keystroke
        // 会被丢掉（F0 的 layout 自检同样如此），这时退一步直接派发键位表里对应的 action（仍然走焦点链）
        let press = |cx: &mut AsyncApp, keys: &str| {
            let ks = Keystroke::parse(keys).expect("键位写错了");
            let _ = cx.update_window(any, |_, window, cx| {
                if window.is_window_active() {
                    window.dispatch_keystroke(ks, cx);
                    return;
                }
                let ctx = window.context_stack();
                let action = crate::actions::keymap()
                    .into_iter()
                    .filter(|s| s.keys == keys)
                    .filter(|s| s.context.map_or(true, |c| ctx.iter().any(|k| k.contains(c))))
                    .max_by_key(|s| s.context.is_some())
                    .map(|s| s.action);
                match action {
                    Some(a) => window.dispatch_action(a, cx),
                    None => eprintln!("[selftest] {keys} 在当前焦点下没有绑定"),
                }
            });
        };
        // 改数据 → 下一帧画完的耗时
        let timed = |cx: &mut AsyncApp, what: &'static str, f: Box<dyn FnOnce(&mut AppState, &mut gpui::Context<AppState>)>| {
            let st = state.clone();
            let _ = cx.update_window(any, move |_, window, cx| {
                let t = Instant::now();
                st.update(cx, |s, cx| f(s, cx));
                window.on_next_frame(move |_, _| eprintln!("[selftest] 性能 {what}：{:.1}ms 到下一帧", t.elapsed().as_secs_f64() * 1000.0));
            });
        };
        let focused = |cx: &mut AsyncApp| -> (bool, bool) {
            cx.update_window(any, |_, window, cx| {
                let v = sidebar.read(cx);
                (v.debug_list_focused(window), v.debug_search_focused(window, cx))
            })
            .unwrap_or((false, false))
        };

        let active = cx.update_window(any, |_, window, _| window.is_window_active()).unwrap_or(false);
        eprintln!("[selftest] 窗口{}是 key window（{}）", if active { "" } else { "不" }, if active { "按真实键位" } else { "按键位表派发 action" });
        // 没有任何焦点时 dispatch_action 只从分发树的根派发，根视图上的 action 收不到（真实启动时工作区会拿焦点；
        // 假 HOME 里一个标签都没有时不会）。先把焦点给侧栏列表，后面的全局键才有落脚点
        let _ = cx.update_window(any, |_, window, cx| sidebar.update(cx, |v, _| v.debug_focus_list(window)));
        pause(200).await;
        let initial = print(cx, "初始（状态视图）");
        if n > 0 && initial == 0 {
            failures.push("有会话但侧栏一行都没有".into());
        }

        // ---- 视图切换 + 性能 ----
        timed(cx, "切到项目视图", Box::new(|s, cx| s.update_prefs(cx, |p| p.sidebar.view = "project".into())));
        pause(600).await;
        print(cx, "项目视图");
        let big = cx.read_entity(&state, |s, _| s.sessions.iter().find(|m| m.git_root.ends_with("big-repo")).map(|m| m.git_root.clone())).ok().flatten();
        if let Some(key) = big.clone() {
            let before = print(cx, "展开 big-repo 之前");
            timed(cx, "展开 200 条的项目组", Box::new(move |s, cx| s.update_prefs(cx, |p| p.sidebar.proj_collapsed = crate::sidebar::collapse::toggle_project_collapsed(&p.sidebar.proj_collapsed, &key, false))));
            pause(600).await;
            let after = print(cx, "展开 big-repo");
            if after < before + 150 {
                failures.push(format!("展开 big-repo 后行数 {before} → {after}，没展开"));
            }
            // 对照：同样的循环（refresh + 16ms 间隔）但不滚 —— 扣掉后台窗口被节流 / 强制绘制本身的开销
            let t = Instant::now();
            for _ in 0..30 {
                let _ = cx.update_window(any, |_, window, _| window.refresh());
                pause(16).await;
            }
            eprintln!("[selftest] 性能 对照：不滚、同样循环 30 次：{:.0}ms", t.elapsed().as_secs_f64() * 1000.0);
            // 滚动：每 16ms 往下滚 400px，共 30 次（ListState::scroll_by，和滚轮走同一个状态）
            let t = Instant::now();
            for _ in 0..30 {
                let _ = cx.update_window(any, |_, window, cx| {
                    sidebar.update(cx, |v, cx| v.debug_scroll_by(400.0, cx));
                    window.refresh();
                });
                pause(16).await;
            }
            eprintln!("[selftest] 性能 滚 30 次（每次 400px）：{:.0}ms（含每次 16ms 间隔）", t.elapsed().as_secs_f64() * 1000.0);
        }
        timed(cx, "切回状态视图", Box::new(|s, cx| s.update_prefs(cx, |p| p.sidebar.view = "status".into())));
        pause(600).await;
        print(cx, "状态视图");
        timed(cx, "改排序为消息数（历史不分段）", Box::new(|s, cx| s.update_prefs(cx, |p| p.sidebar.sort = "count".into())));
        pause(400).await;
        print(cx, "按消息数");
        timed(cx, "排序改回最近活动", Box::new(|s, cx| s.update_prefs(cx, |p| p.sidebar.sort = "recent".into())));
        pause(400).await;

        // ---- ⌘⇧F 搜索 ----
        press(cx, "cmd-shift-f");
        pause(300).await;
        if !focused(cx).1 {
            failures.push("⌘⇧F 之后搜索框没拿到焦点".into());
        }
        let _ = sidebar.update(cx, |v, cx| v.debug_set_query("big-repo", cx));
        pause(300).await;
        let hits = print(cx, "搜索 big-repo");
        if big.is_some() && hits == 0 {
            failures.push("搜 big-repo 没结果".into());
        }
        press(cx, "down");
        pause(300).await;
        let (list_f, _) = focused(cx);
        let sel = cx.read_entity(&sidebar, |v, _| v.debug_selected()).ok().flatten();
        print(cx, "搜索框 ↓");
        if !list_f || sel.is_none() {
            failures.push(format!("搜索框按 ↓ 没进列表（列表焦点 {list_f}，选中 {sel:?}）"));
        }
        let _ = sidebar.update(cx, |v, cx| v.debug_set_query("", cx));
        pause(300).await;

        // ---- ↑↓ / ←→ / Enter ----
        let s0 = cx.read_entity(&sidebar, |v, _| v.debug_selected()).ok().flatten();
        press(cx, "down");
        pause(200).await;
        press(cx, "down");
        pause(200).await;
        let s1 = cx.read_entity(&sidebar, |v, _| v.debug_selected()).ok().flatten();
        print(cx, "↓↓");
        if n > 2 && s0 == s1 {
            failures.push("↓ 没移动选中".into());
        }
        press(cx, "up");
        pause(200).await;
        print(cx, "↑");
        let rows_before = print(cx, "← 之前");
        press(cx, "left");
        pause(300).await;
        let rows_after = print(cx, "← 折叠所在组");
        press(cx, "right");
        pause(300).await;
        let rows_back = print(cx, "→ 展开所在组");
        if rows_after >= rows_before && rows_before > 0 {
            eprintln!("[selftest] 注意：← 没减少行数（选中项可能在不分段的历史组里，那里是 no-op）");
        }
        if rows_back != rows_before {
            failures.push(format!("←→ 之后行数没回来：{rows_before} → {rows_after} → {rows_back}"));
        }
        press(cx, "space");
        pause(300).await;
        print(cx, "Space 悬停卡");
        press(cx, "space");
        pause(200).await;
        let tabs_before = cx.read_entity(&state, |s, _| crate::workspace::model::collect_all_tab_ids(&s.workspace.state.root).len()).unwrap_or(0);
        press(cx, "enter");
        pause(800).await;
        let (tabs_after, active_sid) = cx
            .read_entity(&state, |s, _| (crate::workspace::model::collect_all_tab_ids(&s.workspace.state.root).len(), s.workspace.active_tab().and_then(|t| t.session_id.clone())))
            .unwrap_or_default();
        print(cx, "Enter 打开");
        if active_sid.is_none() || active_sid != cx.read_entity(&sidebar, |v, _| v.debug_selected()).ok().flatten() {
            failures.push(format!("Enter 没打开选中的会话（标签 {tabs_before} → {tabs_after}）"));
        }

        // ---- 右键菜单 / 显示选项 / Esc ----
        if let Some(id) = active_sid.clone() {
            let _ = cx.update_window(any, |_, window, cx| sidebar.update(cx, |v, cx| v.debug_open_menu(&id, window, cx)));
            pause(300).await;
            print(cx, "右键菜单");
            press(cx, "escape");
            pause(300).await;
            if cx.read_entity(&sidebar, |v, cx| v.debug_summary(cx)).unwrap_or_default().contains("popup=true") {
                failures.push("Esc 没关右键菜单".into());
            }
        }
        let _ = cx.update_window(any, |_, window, cx| sidebar.update(cx, |v, cx| v.debug_open_options(window, cx)));
        pause(300).await;
        print(cx, "显示选项菜单");
        press(cx, "escape");
        pause(300).await;

        // ---- ⌘L：先折叠当前会话所在的组、搜个别的，再定位 ----
        let _ = sidebar.update(cx, |v, cx| v.debug_set_query("没有这个会话", cx));
        timed(cx, "折叠「已打开」", Box::new(|s, cx| s.update_prefs(cx, |p| p.sidebar.group_collapsed.push("opened".into()))));
        pause(300).await;
        print(cx, "⌘L 之前（搜索无结果 + 已打开折叠）");
        press(cx, "cmd-l");
        pause(800).await;
        print(cx, "⌘L");
        let (sel, (list_f, _)) = (cx.read_entity(&sidebar, |v, _| v.debug_selected()).ok().flatten(), focused(cx));
        if sel != active_sid || !list_f {
            failures.push(format!("⌘L 没选中当前会话或没拿焦点（选中 {sel:?}，列表焦点 {list_f}）"));
        }
        if cx.read_entity(&state, |s, _| s.prefs.sidebar.group_collapsed.contains(&"opened".to_string())).unwrap_or(true) {
            failures.push("⌘L 没展开被折叠的「已打开」".into());
        }
        press(cx, "escape");
        pause(300).await;
        let (list_f, _) = focused(cx);
        print(cx, "Esc");
        if list_f {
            failures.push("Esc 之后焦点还在侧栏".into());
        }

        // ---- #238 一键展开 / 折叠全部（⌘⇧E）：两种视图各按两次，第二次要和第一次相反 ----
        for view in ["status", "project"] {
            let _ = state.update(cx, |s, cx| s.update_prefs(cx, |p| p.sidebar.view = view.into()));
            pause(300).await;
            let snapshot = |cx: &mut AsyncApp| {
                cx.read_entity(&state, |s, _| {
                    let p = &s.prefs.sidebar;
                    (p.group_collapsed.len(), p.proj_collapsed.iter().filter(|k| !k.starts_with("__expanded__")).count())
                })
                .unwrap_or((0, 0))
            };
            press(cx, "cmd-shift-e");
            pause(400).await;
            let first = snapshot(cx);
            press(cx, "cmd-shift-e");
            pause(400).await;
            let second = snapshot(cx);
            print(cx, &format!("⌘⇧E {view} 两次之后的侧栏"));
            eprintln!("[selftest] ⌘⇧E {view}：第一次后 (状态组折叠数, 项目折叠数)={first:?}，第二次后 {second:?}");
            let (a, b) = if view == "status" { (first.0, second.0) } else { (first.1, second.1) };
            if a == b {
                failures.push(format!("⌘⇧E 在 {view} 视图连按两次没有翻转（{a} → {b}）"));
            }
            // 有折叠的先被全展开（折叠数 0），再全折叠（> 0）；或者反过来
            if a != 0 && b != 0 {
                failures.push(format!("⌘⇧E 在 {view} 视图两次结果都不是「全展开」（{a}, {b}）"));
            }
        }
        let _ = state.update(cx, |s, cx| s.update_prefs(cx, |p| p.sidebar.view = "status".into()));

        // ---- 存活巡检：会话的进程被 kill 之后，侧栏要在几秒内变回「已停止」（不靠任何文件事件）----
        // MAKIT_SELFTEST_KILL=<session_id>:<pid>（假 HOME 里已写好对应的 sessions/<pid>.json，pid 是个活着的子进程）
        if let Ok(spec) = std::env::var("MAKIT_SELFTEST_KILL") {
            if let Some((sid, pid)) = spec.split_once(':').and_then(|(a, b)| Some((a.to_string(), b.parse::<i32>().ok()?))) {
                let running = |cx: &mut AsyncApp| cx.read_entity(&state, |s, _| s.session(&sid).map(|m| m.running)).ok().flatten();
                pause(4000).await; // 等一轮巡检把初始状态合进来
                let before = running(cx);
                unsafe { libc::kill(pid, libc::SIGKILL) };
                let t = Instant::now();
                let mut after = running(cx);
                while after == Some(true) && t.elapsed() < Duration::from_secs(8) {
                    pause(250).await;
                    after = running(cx);
                }
                eprintln!("[selftest] 存活巡检：kill 前 running={before:?}，kill 后 {:.1}s running={after:?}", t.elapsed().as_secs_f64());
                if before != Some(true) {
                    failures.push(format!("存活巡检：kill 之前会话就不是运行中（{before:?}），测试前提不成立"));
                } else if after != Some(false) {
                    failures.push(format!("存活巡检：进程 kill 之后 8 秒内会话仍显示运行中（{after:?}）"));
                }
            }
        }

        // ---- 后台刷新时的开销：运行状态合并（没变化）不该重建 ----
        let t = Instant::now();
        let _ = state.update(cx, |s, cx| s.sessions_changed(cx));
        eprintln!("[selftest] 性能 一次 sessions_changed（重算分组 + 拍平）：{:.2}ms", t.elapsed().as_secs_f64() * 1000.0);

        let _ = cx.read_entity(&state, |s, _| s.flush());
        if failures.is_empty() {
            eprintln!("[selftest] 通过");
        } else {
            eprintln!("[selftest] 失败：{}", failures.join("；"));
        }
        let _ = cx.update(|cx| cx.quit());
    })
    .detach();
}
