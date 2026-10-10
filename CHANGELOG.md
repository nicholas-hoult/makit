# Changelog

[简体中文](CHANGELOG.zh-CN.md)

All notable changes to makit will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Performance
- Appending to a long conversation no longer re-walks the whole conversation: the cost of each new record in the conversation view is now constant (about 0.04 ms at 5 MB, 50 MB and 200 MB, previously about 1.4 ms, 13 ms and 110 ms) (#269)

## [0.1.2] - 2026-10-10

### Changed
- Session status dots look the same in the sidebar and on pane tabs: shape tells alive from stopped, colour and breathing tell the state (waiting amber and fast, busy blue and slow, idle soft green)
- Pane dividers, the sidebar edge and its titlebar continuation are a darker shade of the background (brightness x0.92 on light themes, x0.6 on dark ones) instead of a grey overlay, so they read thinner and cleaner

### Fixed
- The titlebar segment above the sidebar edge now lights up together with the lower part when hovering or dragging the sidebar resizer

## [0.1.1] - 2026-10-05

First public release (macOS, universal Intel + Apple Silicon package, unsigned). The version number starts at 0.1.1: the `v0.1.0` tag was made before the default language became English and was never announced. A pure Rust native app (GPUI) that replaces the earlier Tauri version.

### Added
- Session management: scans local Claude Code and Codex sessions, groups them by project / status, shows running / waiting for approval / idle / stopped / archived; resume with one click, and recover a session even if its start directory was deleted or renamed (Claude Code)
- Split terminals: tree-shaped splits, tab dragging, closing a tab cleans up the session process; links in the terminal, double-click word selection, font zoom, scrolling feel
- ⌘K command palette: search projects / tasks, filter by project / time / status
- Status notifications: desktop notifications when a session waits for approval or an answer and when a task finishes, plus a notification center (Claude Code only)
- Interface: 21 built-in themes (dark / light, follows the system automatically), import iTerm2 `.itermcolors`; Chinese / English, English by default, switchable to Chinese or following the system in settings
- A home-directory shell opens automatically on first launch; window position and size are remembered
- "Diagnostics" in settings: open the log folder, copy diagnostic information; logs are kept by local date for the last 7 days
- Third-party licence list shipped with the app (`THIRD_PARTY_LICENSES.md`)

### Known limitations
- Not signed or notarized: the first launch must be allowed manually (see README)
- Codex sessions can be scanned and resumed, but running / waiting state and notifications are not available
- Linux is planned for 0.2, Windows for 0.3

---

[Unreleased]: https://github.com/nicholas-hoult/makit/compare/v0.1.2...HEAD
[0.1.2]: https://github.com/nicholas-hoult/makit/releases/tag/v0.1.2
[0.1.1]: https://github.com/nicholas-hoult/makit/releases/tag/v0.1.1
