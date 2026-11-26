# Changelog

[简体中文](CHANGELOG.zh-CN.md)

All notable changes to makit will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.27] - 2026-11-24

### Fixed
- The sidebar flashed while typing in a session that was writing output: an update that changed nothing still re-sorted and redrew the whole sidebar. Now it only does that when a session's content actually changed.

## [0.2.26] - 2026-11-24

### Added
- Sidebar session icons show each session's state: an open session wears a ring in its state colour (busy, waiting, idle, stopped), and a session that is not open is dimmed. A waiting session is always lit, even when it is not open, so it cannot be missed.
- The model icon is always shown in the sidebar; the "show model icon" option was removed.

### Changed
- A project group's default expanded or collapsed state is fixed the first time the group appears, so message activity no longer makes a group expand or collapse by itself.
- Titlebar: the sidebar toggle and bell sit further from the traffic lights and closer to each other. Tab separators and the context menu border are lighter.

## [0.2.25] - 2026-11-21

### Added
- The local-session link gained `stop` (kill the session's tab's process, keep the tab itself, marked stopped -- the same thing the sidebar's own "Stop" does) and `close` (kill the process and drop the tab outright, same as the sidebar's "Close"). Both refuse a session that is busy or waiting for approval with a structured reason unless the caller passes `force: true`, since a remote caller has no confirm dialog to answer. Each entry in `sessions` now also carries `tab_state` ("open" / "stopped" / "none") so a caller can tell which of the two even applies.

## [0.2.24] - 2026-11-20

### Added
- Each session in the local-session link's `sessions` list now carries `has_tab`: whether makit currently has a tab for it at all, so a caller can batch-check which sessions `resume { allow_new_tab: false }` would actually revive without calling it on each one and causing side effects (#299).
- The local-session link's `open` gained an optional `trust`: pre-accepts Claude Code's own trust-this-directory dialog for a new session's directory, but only when it is under one of the user's own configured auto-trust roots (a new setting, default empty); otherwise it refuses with a structured reason instead of opening anything untrusted. `open` and `resume` both also gained an optional `permission_mode`, passed straight through as Claude Code's own `--permission-mode` (#D-2168).
- ⌘K's content search is now parallel across CPU cores and debounced off the UI thread, with an incremental, mtime-based refresh instead of rebuilding from scratch (#212).

### Fixed
- The local-session link's `input` with `kind:"text"` could reply `ok` while the text actually sat unsent in the input box (an Enter landing at the wrong moment got swallowed by the TUI); it now confirms against the session's own conversation before replying, retries once with a bare Enter, and reports a structured reason instead of a false success. Text appended to a session that is busy running a turn skips this check, since it is not expected to show up on any short timescale (#D-2161).
- The local-session link's `resume` reported "session not found" for a session that was archived but still on disk: the check for whether it existed went through the same list that archiving filters out of, instead of looking the session up on disk directly like it was always supposed to (#D-2188).
- Updating in place could look stuck for about a minute with no feedback (the "downloading" toast disappeared long before the download actually finished), and if the session driving the update was itself the one waiting to be resumed after the restart, nothing was left alive to ever resume it. Restarting could also silently drop sessions that were running in the restarting instance's own tabs from the next `last_running` snapshot (#D-2149, #D-2150).
- ⌘K jumping to a search match inside a long conversation could land far from the right spot, because the scroll happened before the list had been laid out once with real row heights (#212).

## [0.2.21] - 2026-11-19

### Added
- The local-session link's `resume` gained an optional `allow_new_tab`: the caller can ask makit to only reuse a tab the session already has and refuse (with a structured reason) instead of opening a new one when there is nothing to reuse (#299).

### Fixed
- The installed Claude Code hook script hung for ~4s on every single hook event (until Claude Code's own hook timeout killed it): macOS's bundled `nc` never exits once its peer closes the connection. The script now calls a `hook` subcommand on makit's own executable instead (#215).
- Resuming an ended session through the local-session link could fail with "did not start a process" for every session right after a restart: the check for "does this tab need reviving" mistook "nothing has been built for it yet this launch" for "already running fine" (#299).
- The detached helper that swaps in a downloaded update and relaunches makit now runs in its own process group instead of makit's, so it keeps running even if something signals makit's whole process group while it is still waiting for the old process to exit (#299).

## [0.2.20] - 2026-11-19

### Added
- The local-session link gained two ops: `last_running` (a lightweight, read-only snapshot of the sessions that were running right before the last quit -- id, name, cwd, last active time) and `update` (`check` reports what the running instance currently knows about a newer release; `install` restarts into it, with an optional `after_restart: {session_id, text}` to type something into a session once it goes idle again after the restart) (#318).

### Fixed
- The local-session link's `resume` and `open` used to reply `ok` before the terminal actually finished starting, so a fast follow-up `screen` or `input` request could see "session not open in makit" even though the reply said success; they now wait for the process to really be running before replying, and report a real error when it fails to start (#318).
- The capacity cap's memory check only counted strictly-free pages, which misread a machine with a lot of reclaimable cache as critically low on memory and refused every `resume` / `open`; it now counts available memory (free plus reclaimable cache), matching what `memory_pressure` reports (#318).

### Changed
- Dropped Moonwalk and the two Modus Tinted color themes (their upstream licensing wasn't clear enough to keep shipping them): 18 built-in themes now, down from 21. Anyone who had one selected is switched to the closest-looking survivor on next launch, with a one-time notice explaining why (#255).

## [0.2.19] - 2026-11-19

### Added
- A session the capacity cap put to sleep now shows as such in the sidebar (a distinct tooltip and a small badge), instead of looking like a plain manual stop (#316).

### Fixed
- Resuming a stopped tab (its own button, the "Resume" menu item, Enter, or a terminal's own "press Enter to resume" prompt) and ⌘K's split-open now go through the same capacity cap as opening a fresh session, instead of being able to bypass it (#316).

## [0.2.18] - 2026-11-19

### Added
- The local-session link's `open` / `resume` now carry a structured `capacity_limit` field ("count" / "low_memory" / "high_load") when refused by the capacity cap, so an automated caller can react to which threshold tripped instead of parsing the message (#316).

## [0.2.17] - 2026-11-19

### Added
- Session capacity settings: a cap on how many sessions may run at once, minimum free memory, and maximum 1-minute load are now adjustable (Settings), along with what to do when the cap is hit -- automatically sleep the longest-idle session, ask each time (default), or just refuse (#316).

### Fixed
- Opening or resuming a session while at the capacity cap now actually acts on the chosen policy instead of always refusing: it sleeps the queue's victim and proceeds (auto), or asks first when triggered locally (#316).
- Raised the app's own file-descriptor limit at startup and started logging it periodically, to catch the kind of resource exhaustion that caused #317 earlier.

## [0.2.16] - 2026-11-18

### Fixed
- The Windows build (preview .zip) failed in 0.2.14 because a unix-only dependency was used unconditionally; it now reads as "unavailable" there instead, same as it already did when it could not read the data on an unsupported system. No change on macOS.

## [0.2.14] - 2026-11-18

### Fixed
- Resuming a session through the local-session link no longer opens a second tab for it: it now brings back the tab it still has, whether that tab was explicitly stopped or its process simply exited on its own (#315). Resuming one that turns out to already be running returns a quiet "already running" instead of an error.
- `resume` / `archive` through the local-session link now check whether a session exists directly on disk instead of against the `sessions` list, which is capped and could come back short under heavy load; a session the list happened to leave out is no longer wrongly reported as "not found" (#317).
- `open` / `resume` through the local-session link now refuse with a clear reason when the machine is critically low on free memory or under very high load, instead of reporting success while the new session silently fails to start (#316, #317).

## [0.2.13] - 2026-11-17

### Fixed
- Claude Code sitting at its prompt after a turn ends no longer reads as "needs your approval": replies, the sidebar, and notifications all treat it as idle now (#308).
- Sessions with a reminder no longer jump around in the sidebar as their status is re-checked; needs-reply, running, and idle sessions keep a stable order, only moving when they actually change group (#286).
- A reply sent while a session is busy could have its Enter swallowed after a multi-line paste more often than while idle; it now waits a bit longer before sending Enter in that case.

### Changed
- The "needs you" color is more vivid and no longer looks like a muted warning; the stopped badge is bigger and easier to make out; the sidebar's hover card now follows the pointer instead of staying pinned to the row (#278).

## [0.2.12] - 2026-11-15

### Added
- The local session link's `open` can start a session with its first message already sent, instead of the caller polling for a session id that does not exist until the session writes its first message (#299).
- The local session link can now archive a session (#299).

### Fixed
- A reply sent over the local session link could have its Enter swallowed right after a multi-line paste, leaving the message sitting unsent; it is now sent as its own step after a short pause (#299).
- Idle and stopped sessions now get a corner badge too, and the busy/idle colors were muted to grayscale so only "waiting for you" keeps color; idle sessions used to look just like ones that had never been opened (#278).

### Changed
- A small gap now separates the sidebar-toggle and bell icons in the title bar.

## [0.2.11] - 2026-11-13

### Changed
- Sidebar group counts are now shown only on hover; an option in the display settings ("Always show group counts") turns them back on for every group, including collapsed ones (#304).
- Every status group in the sidebar (Opened, Needs reply, Running, Idle, History) now follows the sort order chosen in settings; before, only Pinned and History did, and Opened always followed on-screen tab order regardless of the setting (#304).

## [0.2.10] - 2026-11-11

### Changed
- Sidebar rows look calmer: rounder corners, a little more room, semibold titles, and the open session is marked by a soft fill alone.
- The tool logo (Claude Code, Codex, opencode) now sits at the start of each row at title size, and the session state is a small badge on its corner: a dot when it waits for you, a ring while it works. Idle sessions have no badge, and a stopped session's logo is dimmed. With "show icon" turned off, the row keeps a plain state icon, drawn only for waiting and busy.
- Status icons in the tabs are quieter too: only waiting and busy are drawn, and they are a bit smaller.
- The app inside the installer now carries its own notarization ticket, so the first launch after dragging it out of the disk image no longer needs the network.

## [0.2.9] - 2026-11-10

### Fixed
- Relative paths that Claude Code prints (such as `supabase/schema.sql`) now open on Cmd+click even after the agent has changed into another folder. They used to resolve against the folder the shell last reported, so the file was not found and nothing happened (#300)

## [0.2.8] - 2026-11-10

### Added
- Local session link (opt-in, off by default): other programs on this Mac can read your sessions and conversations over `~/.makit/link.sock`, and get notified when they change. Turn it on in Settings; nothing leaves the machine (#299)
- A second switch lets those programs act on sessions too: reply, approve or refuse a permission prompt, stop a running turn, press keys, type into the terminal, start a new Claude Code / Codex session in a folder, or resume an ended one. makit shows a toast whenever input arrives this way (#299)
- They can also read a session's terminal screen, so a remote view works with any tool without adapting to it (#299)

### Fixed
- Messages you type while the agent is working (Claude Code takes them into the running turn) now show up in the conversation view; they used to be missing

## [0.2.7] - 2026-11-10

### Added
- The macOS installers are now signed with an Apple Developer ID and notarized by Apple, so they open with a double-click: no more "Open Anyway" in System Settings and no `xattr` workaround (#262)
- The app declares why it may ask for the microphone, camera, contacts, calendar, location, photos and Apple Events, so a program running in the terminal that needs one of them gets a proper system prompt instead of a silent denial

### Changed
- Homebrew: the cask no longer removes the quarantine flag after install, since the app is now notarized
- Because the signing identity changes, macOS may ask again for folder or other permissions the first time you start the signed version after updating from an unsigned one

## [0.2.6] - 2026-11-07

### Fixed
- A long path or URL that a program wrapped itself (real line break plus an indent, as Claude Code does in its replies) is now one link in the terminal, so ⌘-click opens the whole path instead of just one half
- File paths whose names contain full-width brackets (such as `88-【status】title.md`) are now one link in the terminal; they used to be cut short at the bracket, so ⌘-click did nothing or opened the wrong path

## [0.2.5] - 2026-11-02

### Added
- Stop a session without closing its tab: right-click a tab for "Stop session" / "Stop other tabs", or a session in the sidebar for "Stop". The process ends and frees its memory; the tab keeps its place (also across restarts) and shows "Session stopped". Click "Resume" or press Enter to run it again. Stopping a session that is working or waiting on you asks first (#287)
- A tab whose process exited by itself can be run again in place: press Enter in it, or right-click > "Resume session" (#287)
- Pin a tab: right-click > "Pin tab" keeps it at the front of its pane, marked with a pin, also across restarts. Unpinning puts it first among the other tabs. "Close others" and "Close to the right" leave pinned tabs open (#295)
- Batch archive in the sidebar: ⌘-click adds or removes a session from a multi-selection, ⇧-click selects a range, and right-clicking a selected row offers "Archive selected (N)" (or "Unarchive selected (N)" when all of them are archived). Pinned sessions are left alone; if any selected session is running or open in a tab, makit asks once before closing them. Esc clears the selection (#294)

## [0.2.4] - 2026-11-01

### Fixed
- Clicking a session in the sidebar no longer opens a second tab for a session that is already open; the sidebar now decides exactly like ⌘K (#291)
- Opening makit while it is already running now switches to the running window at once, instead of a few seconds of nothing. The shipped app and a dev build (a different app name) can run side by side; two of the same name cannot (#291)

## [0.2.3] - 2026-10-28

### Added
- opencode sessions: they now show up in the sidebar next to Claude Code and Codex, and can be resumed (`opencode -s <id>`), started from the "+" menu, shown as running / busy, and opened in the conversation view. makit reads opencode's SQLite database read-only (#284)
- Environment check: Settings > Diagnostics > "Check environment" looks at the tools and their versions, PATH, proxy variables, the notification hook, data and session folders and the opencode database, and tells you what to fix by hand. Results can be copied with your home folder redacted (#283)

### Changed
- The macOS binary grows by about 180 KB (opencode support; it uses the system SQLite library)

## [0.2.2] - 2026-10-26

### Fixed
- Only one makit can run at a time now. Starting a second one brings the running one to the front and exits, instead of two instances fighting over the hook socket and overwriting each other's saved state. Dev instances with a different data folder are not affected (#291)

## [0.2.1] - 2026-10-25

### Changed
- The update check now runs after every launch and then every hour, instead of about once a day, so a new release shows up the same hour it is published

## [0.2.0] - 2026-10-25

### Added
- Windows build: the release now ships a Windows `.zip` (preview) next to the macOS installers, and it starts (#250, #251, #285)

### Changed
- Session state is shown by shape, not only by colour: a disc with three dots (waiting for you), a ring with an arc (busy), a ring with a tick (idle) and a dashed ring (stopped), in the sidebar and on the tabs. Nothing animates any more (#290, #278)

### Fixed
- makit no longer repaints the whole window five times a second while a session is busy or waiting. Before this change the installed app used about 28% of a core with a few running sessions (#290)
- Codex sessions now show as running in the sidebar while a codex process has them open (they were always listed as stopped). Codex has no status file, so they show as idle, not waiting / busy (#288)
- When a tab's process ends, the terminal now says how: the exit code for a failure, or the signal if it was killed, instead of always "process exited" (#288)
- Windows: `makit.exe` no longer fails at launch with "entry point TaskDialogIndirect not found". The exe now embeds the application manifest that selects comctl32 v6 (and per-monitor DPI awareness), and the release build checks for it (#285)

## [0.1.7] - 2026-10-17

### Fixed
- Quitting with many tabs open is much faster: the sweep for escaped child processes now runs once instead of once per tab. This was why the in-app updater could leave the screen empty for ~15 s before the new version appeared
- The updater's restart helper retries `open`, writes a log to `~/.claude/makit/update-helper.log` and cleans up its temp folder
- Failures to read or save the archive list, and codex tab binding failures, are now written to the log instead of passing silently (#254)

## [0.1.6] - 2026-10-16

### Added
- Interface scale for the sidebar and tab bar: `alt-cmd-=` / `alt-cmd--` / `alt-cmd-0` (zoom in / out / reset). `cmd-+` / `cmd--` still zoom only the terminal font
- Each release now also attaches the universal installer as `makit-<version>.dmg`, so 0.1.4 clients can find and update to it

### Changed
- Tighter sidebar: shorter session rows, 2px between rows, smaller gaps between groups, larger group and project titles with the count on the right

## [0.1.5] - 2026-10-11

### Added
- Windows preview: a `makit-<version>-windows-x64.zip` with `makit.exe` is attached to the release. It is unsigned (SmartScreen asks once: More info, Run anyway) and some features are still placeholders: notifications, the hook channel and the Windows shortcut layout (#251)
- Three macOS installers per release: `-universal` (Intel + Apple Silicon, what the in-app updater and Homebrew use), and `-arm64` / `-x64` at about half the size

### Changed
- The installer is now named `makit-<version>-universal.dmg`. The in-app updater and Homebrew pick it up automatically; **0.1.4 cannot find the new name, so update 0.1.4 by downloading from the Releases page or `brew upgrade --cask makit`**

### Fixed
- The active tab is scrolled back into view when its title changes, so it no longer ends up half hidden behind the pane buttons

## [0.1.4] - 2026-10-11

### Added
- Update notifications: makit checks for a new release in the background (about once a day) and shows an **Update x.y.z** pill in the titlebar. Its menu offers **Update and restart**, **Release notes** and **Skip this version**; Settings has a **Check for updates** button. Only the version information is requested (#200)
- In-place update: downloads the release installer, verifies its SHA-256 against the published digest, swaps the app bundle after exit and relaunches, rolling back if the swap fails. Homebrew installs are pointed to `brew upgrade --cask makit`; anything that cannot be replaced safely opens the release page
- Homebrew: `brew install --cask nicholas-hoult/tap/makit` (#268)

### Changed
- Internal: platform-specific dependencies and code are split per platform, groundwork for Linux and Windows builds (no change in behaviour on macOS) (#184)

## [0.1.3] - 2026-10-10

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

[Unreleased]: https://github.com/nicholas-hoult/makit/compare/v0.1.6...HEAD
[0.1.6]: https://github.com/nicholas-hoult/makit/releases/tag/v0.1.6
[0.1.5]: https://github.com/nicholas-hoult/makit/releases/tag/v0.1.5
[0.1.4]: https://github.com/nicholas-hoult/makit/releases/tag/v0.1.4
[0.1.3]: https://github.com/nicholas-hoult/makit/releases/tag/v0.1.3
[0.1.2]: https://github.com/nicholas-hoult/makit/releases/tag/v0.1.2
[0.1.1]: https://github.com/nicholas-hoult/makit/releases/tag/v0.1.1
