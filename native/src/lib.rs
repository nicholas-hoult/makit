//! makit 的 GPUI 原生界面（#226）。业务逻辑在 makit-core，这里只有界面和界面相关的纯逻辑。
//!
//! ## 模块边界（每个包只在自己的目录里加东西；公共结构的改动集中在 state / actions）
//!
//! | 目录 | 内容 | 归属 |
//! |---|---|---|
//! | `state/` | `AppState` 全局实体（会话列表、工作区模型、持久化偏好）+ 会话增量合并 | F0（公共） |
//! | `actions/` | 全部 action + **唯一的快捷键表** | F0（公共，各包加自己的行） |
//! | `persist/` | `native-state.json` 读写（防抖、原子写）+ 首启从 WebKit localStorage 导入 | F0（公共，各包加字段） |
//! | `theme/` | `Theme` 全局、25 套主题推导、.itermcolors | F0 |
//! | `workspace/` | 递归分割树模型（纯逻辑）+ 工作区视图 | C 工作区 |
//! | `sidebar/` | 侧栏视图 + 分组 / 标题等纯逻辑 | B 侧栏 |
//! | `terminal/` | alacritty 终端视图 + 键位 / 网格 / 调色板纯逻辑 | A 终端 |
//! | `overlays/` | ⌘K、搜索条、详情、设置、toast、右键菜单 | D 浮层 |
//! | `notify/` | hook、通知中心、系统通知 | E 通知 |
//! | `assets.rs` | 打进二进制的图标（AssetSource），各包在自己那段加行 | 公共 |
//! | `app.rs` | 启动流程 + 根视图（全局 action 的落脚点） | F0 |
//! | `selftest.rs` | 无人值守自检（`MAKIT_NATIVE_SELFTEST`） | F0 |
//!
//! 纯逻辑一律带测试（`cargo test --manifest-path native/Cargo.toml`）。

pub mod actions;
pub mod app;
pub mod assets;
pub mod notify;
pub mod overlays;
pub mod perf;
pub mod persist;
pub mod selftest;
pub mod sidebar;
pub mod state;
pub mod terminal;
pub mod theme;
pub mod workspace;
