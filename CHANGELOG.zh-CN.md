# 更新日志

[English](CHANGELOG.md)

本文件记录 makit 的所有重要变更，英文版是 [CHANGELOG.md](CHANGELOG.md)（GitHub Release 说明从它生成），两份内容保持一致。

格式参考 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.0.0/)，版本号遵循[语义化版本](https://semver.org/lang/zh-CN/)。

## [Unreleased]

### 性能
- 往长对话里追加记录时不再把整段对话重走一遍：对话视图里每新增一条记录的耗时变成常数（5 MB、50 MB、200 MB 都约 0.04 ms，之前分别约 1.4 ms、13 ms、110 ms）（#269）

## [0.1.2] - 2026-10-10

### 变更
- 侧栏和标签栏的会话状态点统一：形状区分存活 / 已停止，颜色和呼吸表示状态（等待：琥珀色快呼吸；执行中：蓝色慢呼吸；空闲：浅绿）
- pane 分割线、侧栏边线以及标题栏上与它相连的一段，改成背景色的更深一档（浅色主题亮度 ×0.92，深色主题 ×0.6），不再是叠一层灰，看起来更细更干净

### 修复
- 悬停或拖动侧栏宽度条时，标题栏上那一段现在和下面同时亮起

## [0.1.1] - 2026-10-05

首个公开版本（macOS，Intel + Apple Silicon 通用包，未签名）。版本号从 0.1.1 开始：`v0.1.0` 标签是在默认语言改成英文之前打的，没有正式发布。纯 Rust 原生应用（GPUI），取代更早的 Tauri 版。

### 新增
- 会话管理：扫描本机 Claude Code 和 Codex 会话，按项目 / 状态分组，显示运行中 / 等待审批 / 空闲 / 已停止 / 已归档；一键恢复，启动目录被删或改名也能救回（Claude Code）
- 分屏终端：树形分屏、标签拖拽、关标签彻底清理会话进程；终端内链接、双击选词、字号缩放、滚动手感
- ⌘K 命令面板：搜项目 / 任务，按项目 / 时间 / 状态筛选
- 状态通知：会话等你审批或回答、任务完成时弹桌面通知，通知中心（仅 Claude Code）
- 界面：21 套内置主题（深 / 浅色，自动跟随系统），可导入 iTerm2 `.itermcolors`；中文 / English，默认英文，设置里可切成中文或跟随系统
- 首次启动自动开一个家目录 shell；记住窗口位置和大小
- 设置里的「诊断」：打开日志目录、复制诊断信息；日志按本地日期保存，保留最近 7 天
- 随应用打包第三方许可证清单（`THIRD_PARTY_LICENSES.md`）

### 已知限制
- 没有签名和公证，第一次打开要手动放行（见 README）
- Codex 的会话能扫描、恢复，但看不出运行 / 等待状态，也没有通知
- Linux 计划 0.2，Windows 计划 0.3

---

[Unreleased]: https://github.com/nicholas-hoult/makit/compare/v0.1.2...HEAD
[0.1.2]: https://github.com/nicholas-hoult/makit/releases/tag/v0.1.2
[0.1.1]: https://github.com/nicholas-hoult/makit/releases/tag/v0.1.1
