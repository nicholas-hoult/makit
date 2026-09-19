# Changelog

All notable changes to Makit will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- OSC 9999 状态感知机制（计划中）
- Session 生命周期管理（计划中）
- 跨应用会话持久化（计划中）

## [0.1.0] - 2026-06-21

### Added
- 🖥 树形分屏 Workspace 布局
  - 全局 split 树，支持无限嵌套分屏
  - 拖拽调整分割线，支持方向键导航
  - DOM reparent 架构，PTY 状态永不丢失
- 🔍 Cmd+K 命令面板
  - 层级化搜索（按项目 / 状态分组）
  - 状态分组（运行中/等待审批/空闲/已停止/已归档）
  - 右侧筛选侧栏（项目/时间/状态）
- ⚡ 实时状态监控
  - fs.watch 增量推送（< 500ms 延迟）
  - 五态标识系统
  - Window focus 立即刷新
- 🔗 终端增强
  - Cmd+点击打开路径（支持相对路径、中文、~）
  - OSC 7 cwd 跟踪
  - Cmd+F 终端内搜索
  - IME Shift 符号修复
- 🎯 Session 管理
  - 自动扫描 Claude Code sessions
  - 一键恢复历史会话
  - 项目级分组和归档
- 🎨 UI/UX
  - 原生 macOS 风格
  - 暗色主题
  - 快捷键全覆盖

### Fixed
- 修复 split 分割线拖动功能
  - splitId 格式不匹配问题
  - 闭包导致的状态重置
  - cursor 残留问题

### Known Issues
- Retina 屏幕上终端 cols 偏少（xterm.js 上游问题）
- 目录改名后 session 恢复需要 symlink 修复
- 关闭 tab 后 session 未正确清理（计划 v0.2.0 修复）

---

## Release Notes Template

### [Version] - YYYY-MM-DD

#### Added
- New features

#### Changed
- Changes in existing functionality

#### Deprecated
- Soon-to-be removed features

#### Removed
- Removed features

#### Fixed
- Bug fixes

#### Security
- Security fixes

---

[Unreleased]: https://github.com/yourusername/makit/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/yourusername/makit/releases/tag/v0.1.0
