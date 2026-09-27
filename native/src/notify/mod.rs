//! 通知（E 包）：hook 服务、去重、在屏判定、系统通知、Dock 角标、通知中心。
//!
//! 纯逻辑（带测试）：`model`（四类）、`classify`（分类 + 两路主从）、`deliver`（投递决策 + 在屏判定）、`book`（记录 reducer）。

pub mod book;
pub mod classify;
pub mod deliver;
pub mod model;
