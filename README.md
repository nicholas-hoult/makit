# makit — Claude Code Session 管理器

macOS 原生桌面应用，管理本机所有 Claude Code 会话：浏览、搜索、恢复、分屏、实时状态监控。

**技术栈**：Tauri 2 · Rust · React + TypeScript · xterm.js · 零运行时依赖（自包含 .app）

## 核心特性

### 🖥 对标产品 风格 Workspace
- 全局 split 树，每个 leaf 自带独立 tab bar
- 拖拽 split / 方向键切换 / tab 重排序
- PTY 永不死（DOM reparent 架构）
- 快捷键全覆盖

### 🔍 Cmd+K 命令面板
- 层级化搜索
- 状态分组：⚠等待审批 → ▶运行中 → ⏱最近活跃 → 🗄已归档
- 右侧筛选侧栏（项目/时间/状态/置顶）
- 搜索历史 + 分组折叠 + 记忆筛选

### ⚡ 实时状态
- fs.watch 增量推送（sessions/ 变化 < 500ms 反映）
- 五态标识：等待审批 / 运行中 / 空闲 / 已停止 / 已归档
- window focus 立即刷新

### 🔗 终端增强
- Cmd+点击打开路径（相对/绝对/~/中文/ls -F）
- OSC 7 cwd 跟踪（shell cd 后路径解析跟随）
- Cmd+F 局内搜索（xterm SearchAddon）
- IME Shift 符号修复

## 快捷键

| 按键 | 功能 |
|------|------|
| ⌘K | 全局搜索（命令面板） |
| ⌘F | 当前终端内搜索 |
| ⌘⇧F | 聚焦侧栏 session 搜索 |
| ⌘B | 折叠项目列表 |
| ⌘\\ | 折叠会话列表 |
| ⌘D | 左右分屏 |
| ⌘⇧D | 上下分屏 |
| ⌘T | 新建 shell tab |
| ⌘W | 关闭当前 tab |
| ⌘1~9 | 切到第 N 个 container |
| ⌘⌥ + 方向键 | 按几何位置切相邻 container |

## 快速开始

```bash
# 开发（需要分两步：Vite 已内置不自动启动）
./node_modules/.bin/vite &
cargo tauri dev

# 打包
cargo tauri build --bundles app
# 产物：src-tauri/target/release/bundle/macos/makit.app
```

## 架构概览

```
┌─ 侧栏 ──────────────┬─── workspace（对标产品 风格）────────────────┐
│ 项目列表             │ ┌─ container A ──┬─ container B ──┐     │
│   · ai-quant        │ │ [tab1×][tab2]  │ [tab1×]       │     │
│   · nofx            │ │ 🖥 terminal    │ 🖥 terminal   │     │
│ ─────────────────── │ ├─ container C ──┤               │     │
│ 🔍 搜索  ⚡ ⇅ ↻    │ │ [tab1×]        │               │     │
│ session 列表         │ │ 🖥 terminal    │               │     │
│   · [abc] session1  │ └────────────────┴───────────────┘     │
│   · [def] session2  │                                         │
└──────────────────────┴────────────────────────────────────────┘
```

### 数据模型

```typescript
// 全局只有一棵 split 树
type LayoutNode =
  | ContainerNode    // leaf：自带 tab bar + 终端
  | SplitNode;       // 分割

type ContainerNode = {
  kind: "container";
  id: string;
  tabs: PaneTab[];    // 每个 container 内的 tab
  activeTabId: string;
};
```

### 文件结构

| 文件 | 职责 |
|------|------|
| `workspace-types.ts` | 类型 + 树操作 + layoutTree + 迁移 |
| `useWorkspace.ts` | 状态管理 hook（所有 workspace 操作） |
| `WorkspaceView.tsx` | 全局 split 树渲染 + resizer + DnD |
| `ContainerView.tsx` | 单 container UI（tab bar + 终端） |
| `TerminalManager.ts` | xterm 实例管理（DOM reparent 核心） |
| `Terminal.tsx` | TerminalView（thin wrapper） |
| `CommandPalette.tsx` | Cmd+K 命令面板 |
| `App.tsx` | 主应用（侧栏 + breadcrumb + 快捷键） |
| `src-tauri/src/lib.rs` | Rust 后端（session 扫描 + fs.watch） |
| `src-tauri/src/pty.rs` | PTY 管理 + OSC 7 shell 集成 |

### 持久化

| Key | 存储 |
|-----|------|
| `ccs-workspace` | localStorage · split 树 + tabs |
| `ccs-palette-*` | localStorage · Cmd+K 历史/折叠/筛选 |
| `ccs-theme` | localStorage · 主题 |
| `~/.claude/makit/archived.json` | 文件 · 归档列表 |

## 已知限制

| 项 | 现状 |
|---|---|
| 裸目录名（`Pictures`） | 不识别（避免误匹配普通单词） |
| nohup 子进程 | 看不到（PPID=1 逃出进程树） |
| DMG 打包 | 偶尔 bundle_dmg.sh 失败，.app 正常 |
| 公证签名 | 未做（自用） |
| 终端 cols 偏少（Retina） | xterm.js DPR 测量 bug，见下方说明 |

## 已知问题

### 目录改名后 session 无法 resume

**问题**：项目目录改名后（如 `~/coin-picker` → `~/tradegpt-demo`），旧 session 执行 `claude -r <id>` 报 "No conversation found"。

**根因**：Claude Code 的 session 文件存在 `~/.claude/projects/<encode(路径)>/` 下，`claude -r` 按 `encode(当前cwd)` 查找。目录改名后编码路径不匹配，找不到 session 文件。这是 Claude Code 的设计限制（按路径编码存储，无全局索引）。

**当前方案**：app 在 resume 时自动检测 `cwd` 不存在但 `last_cwd` 存在的情况，自动在 `~/.claude/projects/` 下创建 symlink（新编码 → 旧编码），让 Claude CLI 的查找逻辑通过。用户无感知。

**待 Claude Code 修复**：已提 issue 建议 Claude Code 支持按 session ID 全局查找或 `--session-path` 参数。

### xterm.js Retina DPR 导致 cols 偏少

**问题**：Retina Mac (devicePixelRatio=2) 上，xterm.js 的 FitAddon 测量字符宽度为实际值的 ~2 倍（~15px 而非 ~7.8px），导致终端列数约为预期的一半（如 60 cols 而非 116 cols）。

**表现**：
- 终端文本视觉上填满宽度（渲染正确），但可用列数偏少
- resize 后，旧内容（含硬换行 `\n`）不会 reflow — 这是所有终端的标准行为

**根因**：xterm.js 内部 canvas 渲染器在高 DPR 环境下字符宽度计算存在偏差。相关 issue：
- [xtermjs/xterm.js#4728](https://github.com/xtermjs/xterm.js/issues/4728) — DPR > 1 时字体缩放不正确
- [xtermjs/xterm.js#5847](https://github.com/xtermjs/xterm.js/issues/5847) — WKWebView/Tauri 渲染异常

**状态**：等待 xterm.js 上游修复。Tauri WKWebView 环境下普遍存在此问题。
