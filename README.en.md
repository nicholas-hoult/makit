<h1 align="center">makit</h1>

<p align="center"><b>Make It Happen</b></p>

<p align="center">
  Gather the AI coding sessions scattered across your terminal windows into one interface:<br>
  see which are running, which are waiting for you, and which stopped long ago, and jump back into any of them with one click.
</p>

[简体中文](README.md) | English

<p align="center">
  <a href="#license"><img alt="License: MIT OR Apache-2.0" src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg"></a>
  <img alt="Platform: macOS" src="https://img.shields.io/badge/platform-macOS-lightgrey.svg">
  <img alt="Built with Rust" src="https://img.shields.io/badge/built%20with-Rust-orange.svg">
  <img alt="Status: 0.1" src="https://img.shields.io/badge/version-0.1-green.svg">
</p>

<p align="center">
  <a href="#install">Install</a> ·
  <a href="#quick-start">Quick start</a> ·
  <a href="#keyboard-shortcuts">Shortcuts</a> ·
  <a href="#what-it-touches-on-your-machine">Privacy</a> ·
  <a href="#build-from-source">Build from source</a>
</p>

A native app (Rust + GPUI, not an Electron wrapper). 0.1 ships on macOS only; Linux and Windows are in the works. Supports **Claude Code** and **Codex**.

⭐ **If you find it useful, a Star or a Fork is very welcome.** If something feels off or you want a feature, just open an [Issue](https://github.com/nicholas-hoult/makit/issues). PRs are welcome too.

<p align="center">
  <img src="screenshots/dark.jpg" alt="makit: a session list grouped by time on the left, split terminals on the right" width="900">
</p>

> **0.1: three things up front**
> - macOS only (a universal package for Intel + Apple Silicon). Linux is planned for 0.2, Windows beta for 0.3.
> - **Not signed.** You have to allow it manually the first time you open it (steps below). Signing and notarization will wait until there is a developer account.
> - You need Claude Code or Codex already installed on your machine. makit does not replace them; it only manages their sessions.

---

## What it solves

Say you have three projects, each with an AI session attached. A is waiting for you to approve a refactor, B just finished running its tests, and C's bug is still being fixed. The terminal windows are scattered everywhere, and the only way to tell which is still alive and which is waiting for you is to open them one by one.

makit turns that into a single screen:

- **Session list** — scans the sessions already on your machine, groups them by project, and shows each one's state (running / waiting for approval / idle / stopped / archived)
- **One-click resume** — pick a past session and keep chatting, back in its original working directory (Claude Code sessions can be rescued even if the original directory was deleted)
- **Split terminals** — tree-style splits you can resize by dragging, so one screen keeps an eye on several sessions
- **Status notifications** — a desktop notification when a session is waiting for you (Claude Code only)

---

## Features

- **Light** — not a wrapped web page: pure Rust + GPUI native rendering. The installer is 7.7 MB (single-architecture `.dmg`); the universal Intel + Apple Silicon package is about 15 MB
- **Fast** — about 0.55 s from launch to sessions listed in the sidebar; idle memory about 75 MB, the same with 1200 sessions (data and method in "Performance" below)
- **Chinese / English** — the interface is in English by default; switch to Chinese or "Follow system" in Settings, taking effect immediately
- **Beautiful** — 21 curated themes (11 dark, 10 light), automatically follows the system light/dark setting, and can import iTerm2 `.itermcolors` files directly
- **Cross-platform** — the goal is macOS / Linux / Windows. **0.1 ships on macOS only**; Linux is planned for 0.2 and Windows for 0.3. Still in progress, nothing finished yet

### Screenshots

<table>
  <tr>
    <td><img src="screenshots/light.jpg" alt="Light theme: GitHub Light"></td>
    <td><img src="screenshots/catppuccin.jpg" alt="Dark theme: Catppuccin Mocha"></td>
    <td><img src="screenshots/dayfox.jpg" alt="Light theme: Dayfox"></td>
  </tr>
  <tr>
    <td align="center">GitHub Light</td>
    <td align="center">Catppuccin Mocha</td>
    <td align="center">Dayfox</td>
  </tr>
</table>

### Performance

With a sidebar of **1200 sessions** (60 projects × 20 sessions):

<p align="center">
  <img src="screenshots/stress-1200-sessions.jpg" alt="Sidebar with 1200 sessions" width="640">
</p>

| Metric | Measured |
|---|---|
| Installer (`.dmg`, single architecture) | 7.7 MB |
| Executable (single architecture) | 18.9 MB |
| Launch to sessions in the sidebar | about 0.55 s (24 / 600 / 1200 sessions: 567 / 553 / 577 ms) |
| Idle memory (RSS) | 73 MB (24 sessions) → 77 MB (1200 sessions) |

**Test conditions**: Intel Core i7-1068NG7 (2.3 GHz, 4 cores), macOS 26.6.2, release build.

**Notes**:
- These numbers are from an Intel machine. Apple Silicon was not tested and may do better.
- The session files used in the test are tiny (2 lines each). Real sessions are often several MB, so the first scan is slower (after that there is a cache and only the newly appended part is read), which means launch time with real data will be longer than the table above.
- The first time a freshly downloaded program is launched, macOS spends a bit more time verifying it; the table shows the steady-state values after that.
- To reproduce: `python3 native/scripts/fake-sessions.py /tmp/mk 60 20`, then launch the packaged `makit` with `HOME=/tmp/mk`.

---

## Name and story

**makit** has two meanings: *make it* (make it happen) and *make + kit* (a toolkit for getting things done). The tagline is **Make It Happen**.

The origin is very concrete: it's Wednesday afternoon and you're working on the login feature. On screen you have an editor, a terminal, and two or three AI coding sessions: one waiting for your approval, one that just finished, one that has been sitting since yesterday. The next day you can't remember where yesterday's discussion left off, and you have to dig through dozens of conversations for the one about "login"; or the terminal windows are scattered everywhere, and the only way to tell which is still alive and which is waiting for you is to open them one by one.

AI tools are organized around **sessions**, while developers think in **tasks**: "is the login feature done?", not "where is session number 5?". makit starts with the most basic step: gather the scattered sessions into one interface, make their state visible, and let you get back to any of them with one click.

---

## Install

### Download

0.1 only has a universal macOS package; Intel and Apple Silicon share the same file.

**[→ Go to Releases for the latest version](https://github.com/nicholas-hoult/makit/releases)** (0.1 is not released yet; until then you can build it yourself, see "Build from source" at the end)

Download the `.dmg`, open it, and drag makit into Applications.

### First launch (unsigned, this step is required)

Because the package is not signed, double-clicking it directly will be blocked by macOS. Pick either option:

**Option 1: allow it in System Settings**

1. Double-click `makit.app`. You will see a "cannot be opened" prompt; click "Done"
2. Open **System Settings → Privacy & Security** and scroll down; you will see "makit was blocked"
3. Click **Open Anyway**, then confirm once more

**Option 2: remove the quarantine flag from the command line**

If the message says "**is damaged and can't be opened**" (common for packages downloaded through a browser), run:

```bash
xattr -dr com.apple.quarantine /Applications/makit.app
```

Then double-click to open it as usual.

> Every unsigned app needs these steps; it is not specific to makit. If you're not comfortable with that, all the source is right here and you can build it yourself (see the end of this page).

### Prerequisites

makit manages AI CLIs **you have already installed**, so you need at least one of them:

- [Claude Code](https://claude.ai/code)
- [Codex](https://github.com/openai/codex)

If neither is installed, makit will be empty when it opens.

---

## Quick start

1. Open makit; the sessions it found are listed on the left
2. `⌘K` to search for a project or task, Enter to resume
3. `⌘T` opens a new terminal tab; you can also just type `claude` or `codex` in it
4. `⌘D` splits left/right so you can see two sessions on one screen

---

## Keyboard shortcuts

The most common ones:

| Key | Action |
|---|---|
| `⌘K` | Command palette: search projects / tasks, Enter to resume a session |
| `⌘T` | New terminal tab |
| `⌘D` / `⌘⇧D` | Split left/right / top/bottom |
| `⌘L` | Locate the current tab's session in the sidebar |
| `⌘,` | Settings |

<details>
<summary>All shortcuts</summary>

**Sessions**

| Key | Action |
|---|---|
| `⌘K` | Command palette: search projects / tasks, Enter to resume a session |
| `⌘⇧F` | Focus the sidebar search box |
| `⌘L` | Locate the current tab's session in the sidebar |
| `⌘B` | Collapse / expand the sidebar |
| `⌘I` | Notification center |
| `⌘R` | Rescan the session list |
| `⌘,` | Settings |

**Tabs**

| Key | Action |
|---|---|
| `⌘T` | New terminal tab |
| `⌘W` | Close the current tab (along with the processes running in it) |
| `⌘1` ~ `⌘9` | Switch to the Nth tab in the current split |
| `⌘[` `⌘]`, `⌘←` `⌘→` | Previous / next tab (wraps around) |
| `⌃Tab`, `⌃⇧Tab` | Same as above |

**Splits**

| Key | Action |
|---|---|
| `⌘D` | Split left/right |
| `⌘⇧D` | Split top/bottom |
| `⌥⌘1` ~ `⌥⌘9` | Switch to the Nth split |
| `⌥⌘` + arrow keys | Switch to the adjacent split |
| `⌥⌘↩` | Maximize / restore the current split |

**In the terminal**

| Key | Action |
|---|---|
| `⌘F` | Search in the current terminal |
| `⌘=` `⌘-` `⌘0` | Increase / decrease / reset font size |
| `⌘` + click on a path | Open that file or directory (relative paths are resolved against the current directory) |

> **Combinations starting with `⌃` are never intercepted**: `⌃C`, `⌃R`, `⌃L`, `⌃D`, etc. go to the program in the terminal unchanged; makit does not capture them. The only exception is `⌃Tab` for switching tabs.

The full list is in the app's Settings panel.

</details>

---

## Which AI CLIs are supported

Two, but **their capabilities differ**, so please read this before installing:

| | Claude Code | Codex |
|---|---|---|
| Scan existing sessions on this machine | ✅ | ✅ |
| Resume past sessions | ✅ | ✅ |
| Start new sessions | ✅ | ✅ |
| Running / waiting state | ✅ | ❌ always shown as "Stopped" |
| Desktop notifications | ✅ | ❌ |
| Hover detail card (directory, branch, first / last topic) | ✅ | ✅ |
| Resume after the launch directory was deleted | ✅ | N/A (Codex does not index sessions by directory) |

**Why the asymmetry**: makit does not detect the running state itself; it reads the state Claude Code writes to `~/.claude/sessions/<pid>.json`. Codex has no equivalent, so its sessions can be managed and resumed, but you can't tell "running or waiting for you".

The Codex path has not been fully verified on real machines yet. In 0.1, treat it as a **known limitation** rather than a finished feature.

Other CLIs (Gemini, etc.) are not supported for now.

---

## What it touches on your machine

An unsigned app that reads `~/.claude` — you have the right to know exactly what it does. Here is everything:

**Reads**

- `~/.claude/projects/`, `~/.claude/sessions/`, `~/.codex/sessions/` — scans existing sessions to build the list and states

**Writes**

- `~/.claude/makit/` — makit's own data: settings, archive records, icon cache, logs (`logs/`, only the last 7 days are kept, no conversation content), and the socket used for hook communication. Deleting it does not affect your sessions
- `~/.claude/projects/` — **only when you click resume on a Claude Code session whose launch directory was deleted / renamed** does makit create a symbolic link here, so that Claude Code can find the conversation at its original location; it does not write here otherwise
- `~/.claude/settings.json` — **only written when you click the "Install hook" button**. If you don't install the hook, makit changes not a single character of it

**Network**

- Just one request: the first time a tool icon is shown, it downloads the favicon from `anthropic.com` / `openai.com`, stores it locally, and never requests it again
- Nothing is uploaded, there is no telemetry, and there is no account

**Processes**

- For terminals you open in makit, **closing the tab closes everything inside it** — including background processes you started by hand in that tab (a dev server, say). This is deliberate: a tab is a session, and it shouldn't keep burning memory and tokens in the background after you close it. Run processes you want to keep outside makit.

---

## Known issues

There are currently no confirmed blocking issues. If you run into a problem, please open an issue and include the makit version (Settings → About), your macOS version, and the content of "Copy diagnostics".

---

## Roadmap

- [x] Session scanning, resume, state display
- [x] Split terminals, tab dragging
- [x] Command palette and search
- [x] Closing a tab fully cleans up the session process
- [ ] First-launch onboarding
- [ ] Update notifications
- [ ] Linux (0.2)
- [ ] Windows beta (0.3)

---

## Build from source

```bash
git clone https://github.com/nicholas-hoult/makit.git
cd makit

# Development mode (use --fast for everyday work: dependencies are optimized, your own code stays debug; the first build takes a few minutes to twenty-some minutes)
bash native/scripts/dev.sh --fast

# Package the .app (release build, signed locally)
bash native/scripts/bundle.sh
# Output is at .worktrees/.target-gpui/release/bundle/makit.app (when run in the main directory); without .worktrees it is at native/target/release/bundle/
```

Requirements: macOS, Rust stable.

> `src-tauri/` and `src/` in the repo are the pre-0.1 version (Tauri + React). They are frozen and no longer maintained; the current app lives in `native/` and `core/`.

Run the tests:

```bash
cargo test --manifest-path native/Cargo.toml   # main app
cargo test --manifest-path core/Cargo.toml     # session scanning, resume, and other core logic
```

---

## Tech stack

Pure Rust: the UI is [GPUI](https://www.gpui.rs/) (Zed's UI framework), and the terminal core is [alacritty_terminal](https://github.com/alacritty/alacritty). The build output is a self-contained `.app`; Node and Rust are not needed after installing.

---

## Contributing

- ⭐ If you find it useful, give it a **Star**; if you want to change something, **Fork** it and tinker freely
- If you hit a problem or want a new feature, open an [Issue](https://github.com/nicholas-hoult/makit/issues) (the template reminds you to attach the version and diagnostics)
- If you want your changes merged, just open a PR; for larger changes, please open an Issue to discuss first

---

## License

Dual-licensed, at your option:

- [MIT](LICENSE-MIT)
- [Apache License 2.0](LICENSE-APACHE)

Unless you explicitly state otherwise, any contribution you submit to this project is released under the dual license above, with no additional terms.

## Acknowledgements

- [GPUI](https://www.gpui.rs/), [alacritty_terminal](https://github.com/alacritty/alacritty) — the foundation of this app
- [Claude Code](https://claude.ai/code) — the way of working it brought came first, and only then the need to manage that way of working
