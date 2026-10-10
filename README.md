<h1 align="center">makit</h1>

<p align="center"><b>A session manager for Claude Code and Codex.</b></p>

<h2 align="center">Stop losing your AI coding sessions.</h2>

<p align="center">
  makit finds every session on your machine, shows which ones are waiting for you,<br>
  and picks any of them back up in one click, even after the terminal is gone.
</p>

[简体中文](README.zh-CN.md) | English

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
> - macOS only (a universal package for Intel + Apple Silicon). Windows beta is planned for 0.2, Linux for 0.3.
> - **Not signed.** You have to allow it manually the first time you open it (steps below). Signing and notarization will wait until there is a developer account.
> - You need Claude Code or Codex already installed on your machine. makit does not replace them; it only manages their sessions.

---

## Why makit

An AI coding session is a conversation that lives in a terminal window. Close the window, rename the folder, restart the Mac, and the conversation is still on disk, but nothing points at it any more. With a few projects going at once you cannot tell which window is waiting for you, and last Tuesday's session is "somewhere".

makit changes the unit: **the session is the object, not the window.** It reads the session files that Claude Code and Codex already write, so every session is listed whichever terminal it was started in, with its project, state and history, and it stays listed after the window is gone.

Unix solved the same problem for processes long ago with job control. This is that, for AI sessions:

| Unix job control | makit |
|---|---|
| `jobs`: list the jobs and their state | The session list: running / waiting for you / idle / stopped / archived |
| `Stopped (tty input)`: stuck waiting for you | **Waiting for approval / for an answer**, with a notification |
| `fg %n`: bring one back | One-click resume, in its original directory |
| Close the terminal and the job is gone | The session record is on disk: it survives the window, a restart, a renamed folder |

## What it does

### Never lose a session
Scans `~/.claude` and `~/.codex`, groups sessions by project or by state, and searches across all of them. Sessions started in any terminal show up, including the ones that stopped long ago. Nothing to migrate and nothing uploaded.

### Know who is waiting
A state dot on every session, and a desktop notification when one needs approval or an answer (Claude Code; install the hook from Settings). Open ten sessions and look at one place instead of ten windows.

### Pick up where you left off
Resume a session in its original working directory. If that directory was deleted or moved, makit rebuilds the link so a Claude Code session is not "lost".

### Read it back, reflowed *(experimental, off by default)*
Agent output is hard-wrapped at the width it was written. Turn on the reflowable view in Settings and the history is re-laid out from the session file as you resize the window, while the live part stays a real terminal.

### Split terminals
Tree-style splits you resize by dragging, so one screen keeps an eye on several sessions.

## Also

- **Light**: native Rust + GPUI, not a wrapped web page. 7.7 MB installer (single architecture), about 15 MB universal.
- **Fast**: the session list is ready in about 0.6–0.9 s with up to 5,000 sessions; each new record in a very long conversation costs about 0.04 ms (data and method in "Performance" below).
- **Chinese / English**: English by default; switch in Settings or follow the system.
- **Beautiful**: 21 curated themes that follow the system light / dark setting, and iTerm2 `.itermcolors` import.
- **Cross-platform**: the goal is macOS / Linux / Windows. **0.1 ships on macOS only**; Windows is planned for 0.2 and Linux for 0.3.

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

With a sidebar of **1,200 sessions** (60 projects × 20 sessions):

<p align="center">
  <img src="screenshots/stress-1200-sessions.jpg" alt="Sidebar with 1,200 sessions" width="640">
</p>

**Size**

| Metric | Measured |
|---|---|
| Installer (`.dmg`, single architecture) | 7.7 MB |
| Executable (single architecture) | 18.9 MB |
| Installer, universal (Intel + Apple Silicon) | about 15 MB |

**Startup and memory with many sessions**

"Session list ready" is the time from the process start until the session list is available to the sidebar. "Cold" is the first launch (no scan cache), "warm" is median (min–max) over the following launches. Memory is the resident memory 8 s after launch.

| Scenario | Sessions | On disk | Cold | Warm | makit memory |
|---|---|---|---|---|---|
| Small sessions | 24 | 1 MB | 561 ms | 616 (562–745) ms | 55 MB |
| Small sessions | 600 | 3 MB | 778 ms | 594 (560–742) ms | 57 MB |
| Small sessions | 1,200 | 5 MB | 736 ms | 691 (587–753) ms | 59 MB |
| Small sessions | 5,000 | 20 MB | 1,049 ms | 890 (770–1,450) ms | 73 MB |
| 2 MB sessions | 200 | 401 MB | 1,644 ms | 703 (513–1,325) ms | 57 MB |
| 1 MB sessions | 1,000 | 1,004 MB | 3,258 ms | 1,519 (864–4,634) ms | 67 MB |
| 8 terminal panes, each with a shell | 24 | 1 MB | 595 ms | 724 (636–949) ms | 64 MB (80 MB with the 8 shells) |

**One very long conversation** (opening it in the conversation view: first full read, then the cost of each newly appended record, 3 runs):

| Session file | Conversation items | First full read | Each new record afterwards |
|---|---|---|---|
| 5 MB | 2,774 | 24–30 ms | 0.05–0.06 ms |
| 50 MB | 27,733 | 230–236 ms | 0.04–0.05 ms |
| 200 MB | 110,931 | 0.93–0.97 s | 0.04 ms |

**Test conditions**: Intel Core i7-1068NG7 (2.3 GHz, 4 cores), macOS 26.6.2, release build, single architecture. The machine was not completely idle (an IDE and other tools were running), hence the ranges.

**Notes**:
- The first read of a long conversation is linear in its size. Each *later* update is constant-time in the common case (new records appended at the end of the conversation). A rewind, or a reply whose parallel tool-call blocks branch off, rebuilds the view once, which is about 23 ms at 50 MB. Before #269 every update re-walked the whole conversation (about 1.4 ms at 5 MB, about 110 ms at 200 MB). The first-read numbers were re-measured on the same machine at a different time, so they are not a like-for-like comparison with the previous run.
- The session files are synthetic (alternating user / assistant records of about 1.7 KB, linked like a real session), not real conversations.
- "Warm" uses the scan cache makit wrote on the previous run. If a run was stopped before the cache was flushed, the next launch behaves like a cold one, which is why the warm range can be wide.
- These numbers are from an Intel machine. Apple Silicon was not tested and may do better.
- Not measured yet: scrolling and search latency with a huge list, throughput with very large terminal output.
- To reproduce: `bash native/scripts/bench.sh` (uses the packaged app from `bundle.sh`; for one long file use `python3 native/scripts/fake-sessions.py /tmp/mk 1 1 en 51200`, which makes one 50 MB session).

---

## Name and story

**makit** has two meanings: *make it* (make it happen) and *make + kit* (a toolkit for getting things done). The tagline is **Make It Happen**.

The origin is very concrete: it's Wednesday afternoon and you're working on the login feature. On screen you have an editor, a terminal, and two or three AI coding sessions: one waiting for your approval, one that just finished, one that has been sitting since yesterday. The next day you can't remember where yesterday's discussion left off, and you have to dig through dozens of conversations for the one about "login"; or the terminal windows are scattered everywhere, and the only way to tell which is still alive and which is waiting for you is to open them one by one.

AI tools are organized around **sessions**, while developers think in **tasks**: "is the login feature done?", not "where is session number 5?". makit starts with the most basic step: gather the scattered sessions into one interface, make their state visible, and let you get back to any of them with one click.

---

## Install

### Download

0.1 only has a universal macOS package; Intel and Apple Silicon share the same file.

**[→ Download the latest version from Releases](https://github.com/nicholas-hoult/makit/releases/latest)** (`makit-<version>.dmg`, about 15 MB)

Download the `.dmg`, open it, and drag makit into Applications.

### Homebrew

```bash
brew install --cask nicholas-hoult/tap/makit
```

The cask removes the quarantine flag after installing. If macOS still blocks the first launch, follow the steps below.

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

## FAQ

### How is this different from a terminal or a multiplexer?
makit does include split terminals, but it is organised around sessions, not windows: it lists sessions started anywhere, keeps their history, and tells you which one is waiting. Use it alongside the terminal you like.

### Which agents are supported?
Claude Code and Codex. Status and notifications are Claude Code only for now; Codex sessions can be listed and resumed.

### Does it upload my sessions?
No. It reads local files. The update check only requests version information; nothing about your sessions, paths or projects is sent.

### Why is it not signed?
0.1 has no Apple developer account yet. The steps to allow it on first launch are in "Install".

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
- [ ] Windows beta (0.2)
- [ ] Linux (0.3)

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
