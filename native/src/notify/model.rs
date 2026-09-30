//! 通知的数据形状（纯数据，#215 第 2 节「四类通知」）。

use serde::{Deserialize, Serialize};

/// 通知分四类（#215）。「长时间没动静」并入 `NeedsInput`
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// 等待审批（Notification + permission_prompt；状态文件 waiting + waitingFor≠user）
    NeedsPermission,
    /// 等待回答（Notification + idle_prompt 等；状态文件 waiting + waitingFor=user）
    NeedsInput,
    /// 已完成（Stop 且没有后台任务；状态文件 busy → idle）
    TurnComplete,
    /// 出错（StopFailure / 带 error 的事件）
    Error,
}

impl Kind {
    /// 通知中心行里、横幅标题里的叫法。前两个和 Tauri 版一字不差（清单 G 节）
    pub fn label(self) -> &'static str {
        match self {
            Kind::NeedsPermission => "等待审批",
            Kind::NeedsInput => "等待回答",
            Kind::TurnComplete => "已完成",
            Kind::Error => "出错",
        }
    }

    /// 需要你去处理的（审批 / 回答）：只有这两类响铃、会话离开等待时撤回
    pub fn needs_action(self) -> bool {
        matches!(self, Kind::NeedsPermission | Kind::NeedsInput)
    }
}

/// 一个来源（hook / 状态文件）对某个会话说的一件事
#[derive(Clone, Debug, PartialEq)]
pub enum Signal {
    /// 产生一条通知。`message` 是 Claude 的原话（可能为空，状态文件那路永远为空）
    Notify { kind: Kind, message: String },
    /// 离开等待（在终端里批准 / 回答了）：「需要处理」那条标已读 + 撤回横幅；已完成 / 出错不动
    Resolved,
    /// 用户又发了一句 / 会话结束：这个会话的通知全部标已读 + 撤回
    Clear,
}
