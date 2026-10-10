//! System notification backend, picked per platform (#184).
//!
//! macOS: `UNUserNotificationCenter` + Dock badge (`mac.rs`). Other platforms: no system banners yet, only the
//! in-app notification center (`other.rs`); Linux (D-Bus) and Windows (toast) are tracked in the platform matrix.

mod types;
pub use types::{Permission, SysEvent};

#[cfg(target_os = "macos")]
mod mac;
#[cfg(target_os = "macos")]
pub use mac::*;

#[cfg(not(target_os = "macos"))]
mod other;
#[cfg(not(target_os = "macos"))]
pub use other::*;
