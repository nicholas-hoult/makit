//! 系统层：macOS 原生通知框架 `UNUserNotificationCenter`（#215 第 5 节）+ Dock 角标。
//!
//! - 通知 identifier = 会话 id → 同一会话的新横幅自动替换旧的；threadIdentifier = 会话 id → 系统通知中心里按会话叠放
//! - 点横幅：delegate `didReceiveNotificationResponse` → `SysEvent::Clicked(会话 id)` → 跳到那个标签并标已读
//! - 前台：delegate `willPresentNotification` 照样显示（要不要发，投递决策已经判过了）
//! - 撤回：`removeDeliveredNotificationsWithIdentifiers`
//! - 已完成用 passive 打断级别（不亮屏、不打断专注模式）；声音只在需要处理的横幅上
//!
//! **没有 app 身份时不碰这个框架**：`UNUserNotificationCenter.currentNotificationCenter` 在没有 bundle id 的
//! 进程里会直接抛 ObjC 异常把进程带走。`cargo run` / 裸二进制就是这种情况 —— 这时只记通知中心 + Dock 角标，
//! 启动时打一行日志说明。第 0 期实测（#215 TRD）：要 ad-hoc 签名并放在「应用程序」目录才拿得到授权，
//! 不满足时授权状态是 Denied，设置面板据此提示。
//!
//! 没有退回 notify_rust：它在 mac 上是借别的 app 身份（默认 Finder）发的，点了没反应、撤不回，
//! 和通知中心对不上，比不发更糟（#215 问题 8 就是这么来的）。
//!
//! 回调在系统的任意线程上来，统一塞进 channel，由 `Notifier` 在主线程上消费。

use crate::ts;
use std::sync::OnceLock;

use block2::RcBlock;
use futures::channel::mpsc::UnboundedSender;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, ProtocolObject};
use objc2::{class, define_class, msg_send, AnyThread};
use objc2_foundation::{NSArray, NSBundle, NSError, NSObject, NSObjectProtocol, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNAuthorizationStatus, UNMutableNotificationContent, UNNotification, UNNotificationInterruptionLevel,
    UNNotificationPresentationOptions, UNNotificationRequest, UNNotificationResponse, UNNotificationSettings, UNNotificationSound,
    UNUserNotificationCenter, UNUserNotificationCenterDelegate,
};

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

static EVENTS: OnceLock<UnboundedSender<SysEvent>> = OnceLock::new();

fn emit(ev: SysEvent) {
    if let Some(tx) = EVENTS.get() {
        let _ = tx.unbounded_send(ev);
    }
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "MakitNotificationDelegate"]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl UNUserNotificationCenterDelegate for Delegate {
        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present(
            &self,
            _center: &UNUserNotificationCenter,
            _notification: &UNNotification,
            completion: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            // 前台也显示：要不要发在投递决策里已经判过（正看着的不会走到这里）。Sound 只在内容带声音时才响
            completion.call((UNNotificationPresentationOptions::Banner
                | UNNotificationPresentationOptions::List
                | UNNotificationPresentationOptions::Sound,));
        }

        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn did_receive(&self, _center: &UNUserNotificationCenter, response: &UNNotificationResponse, completion: &block2::DynBlock<dyn Fn()>) {
            let id = response.notification().request().identifier().to_string();
            emit(SysEvent::Clicked(id));
            completion.call(());
        }
    }
);

impl Delegate {
    fn new() -> Retained<Self> {
        let this = Self::alloc().set_ivars(());
        unsafe { msg_send![super(this), init] }
    }
}

/// 进程有没有 app 身份（bundle id）。没有就别碰 UNUserNotificationCenter
pub fn bundle_id() -> Option<String> {
    NSBundle::mainBundle().bundleIdentifier().map(|s| s.to_string())
}

pub struct System {
    center: Option<Retained<UNUserNotificationCenter>>,
    /// center 的 delegate 是弱引用，得有人拿着
    _delegate: Option<Retained<Delegate>>,
}

impl System {
    /// 装 delegate、查一次授权状态（结果经 `tx` 回来）。只在主线程调一次
    pub fn start(tx: UnboundedSender<SysEvent>) -> Self {
        let _ = EVENTS.set(tx);
        let Some(bid) = bundle_id() else {
            log::info!(target: "notify", "没有 app 身份（裸二进制 / cargo run）：系统横幅不可用，只记通知中心 + Dock 角标（#215）");
            emit(SysEvent::Permission(Permission::Unavailable));
            return Self { center: None, _delegate: None };
        };
        log::info!(target: "notify", "app 身份 {bid}：用原生 UNUserNotificationCenter");
        let center = UNUserNotificationCenter::currentNotificationCenter();
        let delegate = Delegate::new();
        center.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        let s = Self { center: Some(center), _delegate: Some(delegate) };
        s.refresh_permission();
        s
    }

    pub fn available(&self) -> bool {
        self.center.is_some()
    }

    /// 查授权状态（异步，结果是 `SysEvent::Permission`）
    pub fn refresh_permission(&self) {
        let Some(center) = &self.center else { return };
        let block = RcBlock::new(|settings: std::ptr::NonNull<UNNotificationSettings>| {
            let status = unsafe { settings.as_ref() }.authorizationStatus();
            emit(SysEvent::Permission(map_status(status)));
        });
        center.getNotificationSettingsWithCompletionHandler(&block);
    }

    /// 弹系统询问框（没问过时）；结果经 `SysEvent::Permission` 回来
    pub fn request_permission(&self) {
        let Some(center) = &self.center else { return };
        let block = RcBlock::new(|granted: Bool, err: *mut NSError| {
            if !err.is_null() {
                let e = unsafe { &*err };
                log::warn!(target: "notify", "请求通知授权失败：{}（没签名 / 不在「应用程序」目录时系统直接拒，#215 第 0 期）", e.localizedDescription());
            }
            emit(SysEvent::Permission(if granted.as_bool() { Permission::Granted } else { Permission::Denied }));
        });
        center.requestAuthorizationWithOptions_completionHandler(
            UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound | UNAuthorizationOptions::Badge,
            &block,
        );
    }

    /// 发一条横幅。identifier = 会话 id（同会话替换）。`passive` = 已完成（不打断）
    pub fn post(&self, id: &str, title: &str, subtitle: &str, body: &str, sound: bool, passive: bool) {
        let Some(center) = &self.center else { return };
        let content = UNMutableNotificationContent::new();
        content.setTitle(&NSString::from_str(title));
        if !subtitle.is_empty() {
            content.setSubtitle(&NSString::from_str(subtitle));
        }
        content.setBody(&NSString::from_str(body));
        content.setThreadIdentifier(&NSString::from_str(id));
        if sound {
            content.setSound(Some(&UNNotificationSound::defaultSound()));
        }
        content.setInterruptionLevel(if passive { UNNotificationInterruptionLevel::Passive } else { UNNotificationInterruptionLevel::Active });
        let req = UNNotificationRequest::requestWithIdentifier_content_trigger(&NSString::from_str(id), &content, None);
        let done = RcBlock::new(|err: *mut NSError| {
            if !err.is_null() {
                let e = unsafe { &*err };
                log::warn!(target: "notify", "横幅发送失败：{}", e.localizedDescription());
            }
        });
        center.addNotificationRequest_withCompletionHandler(&req, Some(&done));
    }

    /// 撤回已送达的横幅
    pub fn withdraw(&self, ids: &[String]) {
        let Some(center) = &self.center else { return };
        if ids.is_empty() {
            return;
        }
        let v: Vec<Retained<NSString>> = ids.iter().map(|s| NSString::from_str(s)).collect();
        let arr = NSArray::from_retained_slice(&v);
        center.removeDeliveredNotificationsWithIdentifiers(&arr);
    }
}

fn map_status(s: UNAuthorizationStatus) -> Permission {
    match s {
        UNAuthorizationStatus::NotDetermined => Permission::NotDetermined,
        UNAuthorizationStatus::Denied => Permission::Denied,
        UNAuthorizationStatus::Authorized | UNAuthorizationStatus::Provisional | UNAuthorizationStatus::Ephemeral => Permission::Granted,
        _ => Permission::Unknown,
    }
}

/// Dock 角标（#215：所有未读数；0 = 不显示）。不需要通知授权，裸二进制也有效。只在主线程调
pub fn set_dock_badge(n: usize) {
    let label = (n > 0).then(|| NSString::from_str(&n.to_string()));
    unsafe {
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        if app.is_null() {
            return;
        }
        let tile: *mut AnyObject = msg_send![app, dockTile];
        if tile.is_null() {
            return;
        }
        let _: () = msg_send![tile, setBadgeLabel: label.as_deref()];
    }
}
