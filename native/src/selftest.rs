//! 无人值守自检（锁屏时看不到画面，用它验证逻辑 + 渲染代码不 panic）。
//!
//! - `MAKIT_NATIVE_SELFTEST=layout`：等会话列表加载 → 按真实键位（`window.dispatch_keystroke`，
//!   走整张快捷键表）新建标签、左右分屏、上下分屏、切标签、几何切 pane、最大化 / 还原、关标签，
//!   再从侧栏恢复第一个会话（有的话），每步打印布局摘要，最后写盘退出。
//! - `MAKIT_NATIVE_SELFTEST=sidebar`：侧栏（B 包）的按键 / 分组 / 性能自检，见 `sidebar/selftest.rs`。
//! - `MAKIT_NATIVE_SELFTEST=workspace`：C 工作区包的自检，见 `workspace/selftest.rs`。
//! - `MAKIT_NATIVE_SELFTEST=restore`：启动后打印恢复出来的布局摘要，退出（配合上一步验证布局能保存 / 恢复）。
//!
//! 配合 `MAKIT_NATIVE_FORCE_DRAW=1`（锁屏时逼 GPUI 每帧画）和 `MAKIT_NATIVE_DUMP=<文件>`（终端网格导出）。
//! 输出行都以 `[selftest]` 开头，`[selftest] 通过` / `[selftest] 失败：…` 结尾。

use std::time::Duration;

use gpui::{App, AppContext, Entity, Keystroke, WindowHandle};

use crate::app::Root;
use crate::state::AppState;
use crate::workspace::model::{collect_containers, LayoutNode};

/// 布局摘要：`V(C[t,t*] | H(C[t] / C[t]))`，`*` 是 active tab，`!` 是 active pane，`^` 是最大化
pub fn summary(s: &AppState) -> String {
    fn node(n: &LayoutNode, active: &str, max: Option<&str>) -> String {
        match n {
            LayoutNode::Container(c) => {
                let tabs: Vec<String> = c.tabs.iter().map(|t| format!("{:?}{}", t.kind, if t.id == c.active_tab_id { "*" } else { "" })).collect();
                format!("C{}{}[{}]", if c.id == active { "!" } else { "" }, if Some(c.id.as_str()) == max { "^" } else { "" }, tabs.join(","))
            }
            LayoutNode::Split(sp) => {
                let (op, sep) = if sp.dir == crate::workspace::model::Dir::V { ("V", " | ") } else { ("H", " / ") };
                format!("{op}({}{sep}{})", node(&sp.a, active, max), node(&sp.b, active, max))
            }
        }
    }
    let w = &s.workspace.state;
    node(&w.root, &w.active_container_id, w.maximized_container_id.as_deref())
}

pub fn run(mode: String, handle: WindowHandle<Root>, state: Entity<AppState>, cx: &mut App) {
    // B 侧栏：`MAKIT_NATIVE_SELFTEST=sidebar`（见 sidebar/selftest.rs）
    // 中文在等宽字体里会不会被画成别的字（#239 乱码）：同一段文字用「只有 Menlo」和 mono_font 各排一次，
    // 打出每段用的字体和字形号，和「直接用苹方」排出来的字形号对比
    if mode == "fonts" {
        let _ = handle.update(cx, |_, window, _| {
            let ts = window.text_system().clone();
            let text = "重建原目录";
            let shape = |f: gpui::Font| {
                let run = gpui::TextRun { len: text.len(), font: f, color: gpui::black(), background_color: None, underline: None, strikethrough: None };
                let l = ts.layout_line(text, gpui::px(12.0), &[run], None);
                l.runs
                    .iter()
                    .map(|r| {
                        let fam = format!("{:?}:{}", r.font_id, ts.get_font_for_id(r.font_id).map(|f| f.family.to_string()).unwrap_or_default());
                        let ids: Vec<String> = r.glyphs.iter().map(|g| format!("{:?}", g.id)).collect();
                        format!("{fam}{ids:?}")
                    })
                    .collect::<Vec<_>>()
                    .join(" + ")
            };
            eprintln!("[selftest] 只有 Menlo      : {}", shape(gpui::font("Menlo")));
            eprintln!("[selftest] mono_font       : {}", shape(crate::overlays::style::mono_font()));
            eprintln!("[selftest] 直接用 PingFang SC: {}", shape(gpui::font("PingFang SC")));
            eprintln!("[selftest] 终端字体 text_font: {}", shape(crate::terminal::text_font(window)));
            let mut jb = gpui::font("JetBrains Mono");
            eprintln!("[selftest] 只有 JetBrains Mono: {}", shape(jb.clone()));
            jb.fallbacks = Some(gpui::FontFallbacks::from_fonts(vec!["PingFang SC".into()]));
            eprintln!("[selftest] JetBrains+苹方   : {}", shape(jb));
            // 判定：界面等宽字体必须和终端字体排得一模一样（终端里的中文在真机上是对的，拿它当参照）
            let (ui, term) = (shape(crate::overlays::style::mono_font()), shape(crate::terminal::text_font(window)));
            if ui == term {
                eprintln!("[selftest] 通过：mono_font 和终端字体一致");
            } else {
                eprintln!("[selftest] 失败：mono_font 和终端字体不一致：{ui} ≠ {term}");
            }
            for fam in ["Menlo", "JetBrains Mono", "PingFang SC", "PingFang TC", "Hiragino Sans", "Apple SD Gothic Neo", ".AppleSystemUIFont"] {
                eprintln!("[selftest] 字体编号 {fam:20} = {:?}", ts.resolve_font(&gpui::font(fam)));
            }
        });
        cx.quit();
        return;
    }
    if mode == "sidebar" {
        crate::sidebar::selftest::run(handle, state, cx);
        return;
    }
    // D 浮层自己的自检（overlays/selftest.rs）
    if mode == "overlays" {
        return crate::overlays::selftest::run(handle, state, cx);
    }
    if mode == "notify" {
        return crate::notify::selftest::run(handle.into(), state, cx);
    }
    // C 工作区包的自检（标题栏 / 闪牌 / 分割线 / 右键菜单 / 最大化）
    if mode == "workspace" {
        if let Ok(ws) = handle.read_with(cx, |root, _| root.workspace_view()) {
            crate::workspace::selftest::run(handle, state, ws, cx);
        }
        return;
    }
    cx.spawn(async move |cx| {
        let ex = cx.background_executor().clone();
        let pause = move |ms| ex.timer(Duration::from_millis(ms));
        // 等会话列表
        for _ in 0..600 {
            pause(100).await;
            if cx.read_entity(&state, |s, _| s.loaded).unwrap_or(false) {
                break;
            }
        }
        let print = |cx: &mut gpui::AsyncApp, step: &str| {
            let line = cx.read_entity(&state, |s, _| format!("{}（{} 个会话）", summary(s), s.sessions.len())).unwrap_or_default();
            eprintln!("[selftest] {step}：{line}");
            line
        };
        let press = |cx: &mut gpui::AsyncApp, keys: &str| {
            let ks = Keystroke::parse(keys).expect("键位写错了");
            // 用 AnyWindowHandle：typed handle 的 update 会在整个闭包期间占住 Root 实体，
            // 按键派发途中要是触发重绘（要更新 Root）就会 panic「already being updated」
            let any: gpui::AnyWindowHandle = handle.into();
            let _ = cx.update_window(any, |_, window, cx| window.dispatch_keystroke(ks, cx));
        };
        let mut failures: Vec<String> = Vec::new();
        if mode == "restore" {
            pause(1500).await;
            print(cx, "恢复出来的布局");
        } else {
            print(cx, "初始");
            let steps: [(&str, &str); 11] = [
                ("cmd-t", "⌘T 新建标签"),
                ("cmd-t", "⌘T 再建一个"),
                ("cmd-d", "⌘D 左右分屏"),
                ("cmd-shift-d", "⌘⇧D 上下分屏"),
                ("alt-cmd-left", "⌥⌘← 切到左边"),
                ("cmd-1", "⌘1 第一个标签"),
                ("ctrl-tab", "⌃Tab 下一个标签"),
                ("alt-cmd-enter", "⌥⌘↩ 最大化"),
                ("alt-cmd-right", "⌥⌘→ 最大化时切 pane（放大跟着走）"),
                ("alt-cmd-enter", "⌥⌘↩ 还原"),
                ("cmd-w", "⌘W 关当前标签"),
            ];
            let mut last = String::new();
            for (keys, name) in steps {
                press(cx, keys);
                pause(700).await;
                let now = print(cx, name);
                if now == last && keys != "cmd-1" {
                    failures.push(format!("{name} 没有改变布局"));
                }
                last = now;
            }
            let first = cx.read_entity(&state, |s, _| s.sessions.first().map(|m| m.session_id.clone())).ok().flatten();
            if let Some(sid) = first {
                let _ = state.update(cx, |s, cx| {
                    let m = s.session(&sid).cloned().expect("刚读到的会话");
                    s.workspace.open_session(&m.session_id, &m.short_id, &m.cwd, None, None);
                    s.workspace_changed(cx);
                });
                pause(1500).await;
                print(cx, "恢复第一个会话");
            }
            let tabs = cx.read_entity(&state, |s, _| collect_containers(&s.workspace.state.root).iter().map(|c| c.tabs.len()).sum::<usize>()).unwrap_or(0);
            if tabs < 3 {
                failures.push(format!("最后只剩 {tabs} 个标签，预期至少 3 个"));
            }
        }
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
