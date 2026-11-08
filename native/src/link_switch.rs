//! 本机会话接口的开关（#299）：设置里的「允许本机其他程序读取会话」。默认关；开关即时生效，退出时关掉并清理 socket。
//!
//! 接口本身在 `makit_core::link`；这里只管「什么时候开、什么时候关」。

use std::sync::{Arc, Mutex};

use gpui::{App, Entity};
use makit_core::link::{CoreBackend, Handle};

use crate::state::{AppEvent, AppState};

/// 偏好和实际状态对比后该做什么。
#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    Start,
    Stop,
    Keep,
}

/// 纯判断：想开而没开 → 开；不想开而开着 → 关；其余不动。
/// 为什么单独测：开关错一次，要么用户关了却还在监听（以为安全其实没关），要么开了却没起来（别的程序连不上）。
pub fn decide(enabled: bool, running: bool) -> Action {
    match (enabled, running) {
        (true, false) => Action::Start,
        (false, true) => Action::Stop,
        _ => Action::Keep,
    }
}

/// 启动时按偏好决定开不开，并跟着偏好变化即时开关；退出时关掉。在 `AppState::init` 之后调。
pub fn init(state: &Entity<AppState>, cx: &mut App) {
    let running: Arc<Mutex<Option<Handle>>> = Arc::new(Mutex::new(None));
    let sync = {
        let running = running.clone();
        move |enabled: bool| {
            let mut slot = running.lock().expect("link switch lock");
            match decide(enabled, slot.is_some()) {
                Action::Start => *slot = makit_core::link::start(Arc::new(CoreBackend::new())),
                Action::Stop => {
                    if let Some(h) = slot.take() {
                        h.stop();
                    }
                }
                Action::Keep => {}
            }
        }
    };
    sync(state.read(cx).prefs.link_enabled);
    let on_prefs = sync.clone();
    cx.subscribe(state, move |state, ev: &AppEvent, cx| {
        if matches!(ev, AppEvent::PrefsChanged) {
            on_prefs(state.read(cx).prefs.link_enabled);
        }
    })
    .detach();
    cx.on_app_quit(move |_| {
        if let Some(h) = running.lock().expect("link switch lock").take() {
            h.stop();
        }
        async {}
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decide_starts_stops_or_keeps() {
        assert_eq!(decide(true, false), Action::Start);
        assert_eq!(decide(false, true), Action::Stop);
        assert_eq!(decide(true, true), Action::Keep);
        assert_eq!(decide(false, false), Action::Keep);
    }

    #[test]
    fn link_is_off_by_default_and_old_state_files_load_with_it_off() {
        assert!(!crate::persist::NativeState::default().link_enabled);
        // 旧版本的状态文件没有这一项：按关处理，不能因为缺字段而读失败
        let old: crate::persist::NativeState = serde_json::from_str(r#"{"version":1,"reflow_view":true}"#).unwrap();
        assert!(!old.link_enabled);
        assert!(old.reflow_view);
    }
}
