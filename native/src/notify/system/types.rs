//! Types shared by every platform's system-notification backend (#215, #184).

use crate::ts;

/// 系统通知授权状态（设置面板读这个，D 包）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Permission {
    /// 还没查到
    Unknown,
    /// 没问过（第一次要发时会弹询问框）
    NotDetermined,
    /// 用户拒绝了，或包没签名 / 不在「应用程序」目录（#215 第 0 期：这两种情况系统直接拒，不弹框）
    Denied,
    Granted,
    /// 没有 app 身份（裸二进制 / cargo run）：原生通知框架用不了，只记通知中心
    Unavailable,
}

impl Permission {
    pub fn label(self) -> String {
        match self {
            Permission::Unknown => ts!("notify.permission.unknown"),
            Permission::NotDetermined => ts!("notify.permission.not_determined"),
            Permission::Denied => ts!("notify.permission.denied"),
            Permission::Granted => ts!("notify.permission.granted"),
            Permission::Unavailable => ts!("notify.permission.unavailable"),
        }
    }
}

/// 系统回调 → Notifier
#[derive(Clone, Debug, PartialEq)]
pub enum SysEvent {
    /// 点了横幅（会话 id）
    Clicked(String),
    Permission(Permission),
}
