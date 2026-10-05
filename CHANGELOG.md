# Changelog

All notable changes to Makit will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] - 2026-10-05

首个公开版本（macOS，Intel + Apple Silicon 通用包，未签名）。纯 Rust 原生应用（GPUI），取代更早的 Tauri 版。

### Added
- 会话管理：扫描本机 Claude Code 和 Codex 会话，按项目 / 状态分组，显示运行中 / 等待审批 / 空闲 / 已停止 / 已归档；一键恢复，启动目录被删或改名也能救回（Claude Code）
- 分屏终端：树形分屏、标签拖拽、关标签彻底清理会话进程；终端内链接、双击选词、字号缩放、滚动手感
- ⌘K 命令面板：搜项目 / 任务，按项目 / 时间 / 状态筛选
- 状态通知：会话等你审批或回答、任务完成时弹桌面通知，通知中心（仅 Claude Code）
- 界面：21 套内置主题（深 / 浅色，自动跟随系统），可导入 iTerm2 `.itermcolors`；中文 / English，默认英文，设置里可切成中文或跟随系统
- 首次启动自动开一个家目录 shell；记住窗口位置和大小
- 设置里的「诊断」：打开日志目录、复制诊断信息；日志按本地日期保存，保留最近 7 天
- 随应用打包第三方许可证清单（`THIRD_PARTY_LICENSES.md`）

### Known limitations
- 没有签名和公证，第一次打开要手动放行（见 README）
- Codex 的会话能扫描、恢复，但看不出运行 / 等待状态，也没有通知
- Linux 计划 0.2，Windows 计划 0.3

---

[Unreleased]: https://github.com/nicholas-hoult/makit/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/nicholas-hoult/makit/releases/tag/v0.1.0
