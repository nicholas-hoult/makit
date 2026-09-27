//! 浮层（D 包）：⌘K 命令面板、⌘F 搜索条、会话详情、恢复 cwd 对话框、设置 ⌘,、toast、通用右键菜单。
//!
//! F0 只占了位置：action 已在 `actions::overlays` 声明、快捷键已进表，根视图挂的是占位处理（打日志）。
//! 实现时：浮层视图放在这个目录；在根视图（app.rs）里挂成 `deferred(anchored(..))` 覆盖层，
//! 删掉 app.rs 里对应的占位 `.on_action`。持久化的面板状态已在 `NativeState.palette`。
