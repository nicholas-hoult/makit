# Makit

**让 AI 编码助手成为你的开发团队**

Makit 是一个原生 macOS 应用，专为管理多个 AI 编码会话而设计。当你同时运行多个 Claude Code、Codex 或 Gemini 实例时，Makit 帮你组织、切换和恢复它们，就像管理一个分布式的开发团队。

![macOS](https://img.shields.io/badge/macOS-11.0+-000000?logo=apple)
![Tauri](https://img.shields.io/badge/Tauri-2.0-24C8DB?logo=tauri)
![License](https://img.shields.io/badge/license-MIT-blue)

---

## 为什么需要 Makit？

### 问题：AI 会话管理的混乱

当你的开发流程依赖 AI 助手时：
- **项目 A** 正在等待你审批一个复杂重构
- **项目 B** 的 agent 刚跑完测试，需要查看结果
- **项目 C** 的紧急 bug 修复正在进行中
- 但你的终端窗口散落各处，不知道哪个在干什么

你需要在多个终端标签页之间疯狂切换，不知道哪个 session 还活着，哪个已经完成，哪个需要你的注意。

### 解决方案：统一的会话中心

Makit 将所有 AI 会话集中到一个界面：

**🔍 一眼看到所有状态**  
实时显示每个 session 的状态：运行中、等待审批、已完成、空闲

**⚡ 快速恢复和切换**  
`Cmd+K` 打开搜索，输入项目名或任务描述，回车即可恢复任何历史会话

**📊 分屏工作区**  
同时监控多个 AI agent：左边跑测试，右边写代码，上边审查，下边调试

**🎯 零打断**  
AI 完成任务或遇到问题时，桌面通知立即提醒你，无需轮询

---

## 核心特性

### 智能会话管理
- **自动发现**：扫描 `~/.claude/projects/` 下所有会话，实时同步状态
- **状态标识**：五种状态一目了然（运行中 / 等待审批 / 空闲 / 已停止 / 已归档）
- **快速搜索**：按项目、任务、时间、状态筛选，支持模糊搜索

### 专业级终端
- **分屏布局**：树形分屏，支持拖拽调整和方向键导航
- **多标签管理**：每个分屏独立 tab bar，可重排序和跨屏拖拽
- **路径识别**：`Cmd+点击` 直接打开文件和目录（支持相对路径、中文、~）
- **持久化 PTY**：会话不丢失，分屏移动时终端状态完整保留

### 实时通知
- 集成 Claude Code 的 hook 系统，会话状态变化立即推送
- 支持多 AI provider（Claude、Codex、Gemini）

---

## 快速开始

### 安装

**方式 1：下载发行版**（推荐）
```bash
# 下载最新 .dmg
open https://github.com/yourusername/makit/releases/latest

# 拖拽到应用程序文件夹
```

**方式 2：从源码构建**
```bash
git clone https://github.com/yourusername/makit.git
cd makit
pnpm install

# 开发模式
pnpm dev

# 构建生产版本
pnpm tauri build
```

### 使用

1. **启动 Makit**  
   应用会自动扫描你的 Claude Code sessions

2. **打开命令面板**  
   按 `Cmd+K`，搜索项目或任务

3. **创建新会话**  
   选择项目 → 点击 "新会话" → 自动启动 `claude` 或 `codex`

4. **分屏工作**  
   `Cmd+D` 左右分屏，`Cmd+Shift+D` 上下分屏

---

## 快捷键

| 按键 | 功能 |
|------|------|
| `Cmd+K` | 全局搜索 / 命令面板 |
| `Cmd+T` | 新建 Shell 标签 |
| `Cmd+W` | 关闭当前标签 |
| `Cmd+D` | 左右分屏 |
| `Cmd+Shift+D` | 上下分屏 |
| `Cmd+Opt+方向键` | 切换到相邻分屏 |
| `Cmd+1~9` | 切换到第 N 个分屏 |
| `Cmd+F` | 在当前终端内搜索 |
| `Cmd+B` | 折叠/展开项目列表 |

---

## 技术栈

**前端**：React + TypeScript + Vite  
**终端**：xterm.js + 自定义增强（路径识别、OSC 7 跟踪）  
**后端**：Tauri 2 + Rust  
**PTY 管理**：portable-pty + tokio  
**文件监听**：notify (fsevents)

**零运行时依赖** — 构建产物为自包含的 `.app`，不需要安装 Node.js 或 Rust

---

## 架构概览

```
┌─ 侧栏 ──────────────┬─── Workspace ────────────────────────┐
│ 📁 项目列表          │ ┌─ Container A ─┬─ Container B ───┐  │
│   · ai-project      │ │ [tab1][tab2]  │ [tab1][tab2×]   │  │
│   · web-app         │ │ 🖥 Terminal   │ 🖥 Terminal     │  │
│ ───────────────────  │ ├─ Container C ─┤                 │  │
│ 🔍 搜索  ⚡ 筛选    │ │ [tab1×]       │                 │  │
│ Session 列表         │ │ 🖥 Terminal   │                 │  │
│   ▶ 运行中 (2)      │ └───────────────┴─────────────────┘  │
│   ⚠ 等待审批 (1)    │                                      │
│   ⏱ 最近活跃 (5)    │                                      │
└──────────────────────┴──────────────────────────────────────┘
```

---

## 路线图

- [x] 基础会话管理和终端
- [x] 分屏布局和拖拽
- [x] 实时状态监控
- [x] 命令面板和搜索
- [ ] **OSC 9999 状态感知**（Agent 主动上报状态）
- [ ] Session 生命周期管理（关闭 tab 清理 PTY）
- [ ] 会话持久化和恢复（跨应用重启）
- [ ] 多 AI provider 统一接口
- [ ] 跨设备同步（云端会话）
- [ ] Windows/Linux 支持

---

## 贡献

欢迎提交 Issue 和 Pull Request！

开发环境要求：
- macOS 11.0+
- Rust 1.70+
- Node.js 18+
- pnpm 8+

```bash
# 安装依赖
pnpm install

# 启动开发服务器
pnpm dev

# 运行测试
pnpm test

# 构建
pnpm tauri build
```

---

## 许可证

MIT License - 详见 [LICENSE](LICENSE) 文件

---

## 致谢

- [xterm.js](https://xtermjs.org/) - 强大的终端模拟器
- [Tauri](https://tauri.app/) - 轻量级桌面应用框架
- [Claude Code](https://claude.ai/code) - 启发了这个项目的诞生

---

**Made with ❤️ for developers who work with AI assistants**
