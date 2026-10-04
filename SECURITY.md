# 安全策略 / Security Policy

[中文](#中文) · [English](#english)

## 中文

### 支持的版本

只支持最新发布的版本。请先升级到最新版再确认问题是否仍然存在。

### 报告漏洞

请**不要**在公开 Issue 里提安全问题。请使用 GitHub 仓库 Security 标签页里的 **Report a vulnerability**（Security Advisory）私下报告。请尽量写明：makit 版本、macOS 版本、复现步骤和影响范围。

### makit 会碰你哪些东西

这是评估影响范围时的参考，与 README 的「它会碰你哪些东西」一致。

**读取**

- `~/.claude/projects/`、`~/.claude/sessions/`、`~/.codex/sessions/`：扫描已有会话，用来生成列表和状态。

**写入**

- `~/.claude/makit/`：makit 自己的数据，包括设置、归档记录、图标缓存、日志（`logs/`，只留最近 7 天，不含对话内容）、hook 通信用的 socket。
- `~/.claude/projects/`：只有用户对「启动目录已被删除 / 改名」的 Claude Code 会话点了恢复，makit 才会在这里建一个符号链接；其余时候不写。
- `~/.claude/settings.json`：只有用户点了「安装 hook」按钮才会写；不装 hook，makit 不会改它。

**网络**

- 只有一个请求：第一次显示工具图标时下载 `anthropic.com` / `openai.com` 的 favicon，存在本地，以后不再请求。
- 不上传任何东西，没有遥测，没有账号。

**进程**

- 用户在 makit 里开的终端，关掉标签页就会关掉里面的一切，包括在该标签里手动起的后台进程。

## English

### Supported versions

Only the latest release is supported. Please upgrade and check that the problem still exists.

### Reporting a vulnerability

Please do **not** report security issues in public issues. Use **Report a vulnerability** (GitHub Security Advisory) on the repository's Security tab to report privately. Include the makit version, macOS version, reproduction steps and impact if you can.

### What makit touches

For scoping the impact; this matches the "What makit touches" section of the README (`它会碰你哪些东西`).

**Reads**

- `~/.claude/projects/`, `~/.claude/sessions/`, `~/.codex/sessions/`: scans existing sessions to build the list and statuses.

**Writes**

- `~/.claude/makit/`: makit's own data: settings, archive records, icon cache, logs (`logs/`, last 7 days only, no conversation content), and the socket used for hook communication.
- `~/.claude/projects/`: only when the user chooses to resume a Claude Code session whose launch directory was deleted or renamed, makit creates a symbolic link here; otherwise it does not write.
- `~/.claude/settings.json`: only written when the user clicks "Install hook"; without the hook, makit does not modify it.

**Network**

- A single request: the first time a tool icon is shown, it downloads the favicon from `anthropic.com` / `openai.com`, stores it locally and never requests it again.
- Nothing is uploaded; no telemetry; no account.

**Processes**

- Closing a terminal tab opened in makit closes everything inside it, including background processes the user started manually in that tab.
