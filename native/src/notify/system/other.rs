//! Platforms without a system-notification backend yet: banners are unavailable, the in-app notification center
//! still works. Same surface as `mac.rs`.

use futures::channel::mpsc::UnboundedSender;

use super::{Permission, SysEvent};

pub fn bundle_id() -> Option<String> {
    None
}

pub struct System;

impl System {
    pub fn start(tx: UnboundedSender<SysEvent>) -> Self {
        log::info!(target: "notify", "no system notification backend on this platform: notification center only");
        let _ = tx.unbounded_send(SysEvent::Permission(Permission::Unavailable));
        System
    }

    pub fn available(&self) -> bool {
        false
    }

    pub fn refresh_permission(&self) {}

    pub fn request_permission(&self) {}

    pub fn post(&self, _id: &str, _title: &str, _subtitle: &str, _body: &str, _sound: bool, _passive: bool) {}

    pub fn withdraw(&self, _ids: &[String]) {}
}

/// No Dock badge outside macOS
pub fn set_dock_badge(_n: usize) {}
