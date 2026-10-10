# 参与贡献 / Contributing

[中文](#中文) · [English](#english)

## 中文

谢谢你愿意花时间在 makit 上。小到改一个错别字、大到加一个功能，都欢迎。

### 提 Issue

- 用 [Issue 模板](.github/ISSUE_TEMPLATE/)：问题反馈（`bug`）或功能建议（`feature`）。
- 反馈问题请附上：makit 版本（设置 → 关于）、macOS 版本和芯片、用的是 Claude Code 还是 Codex，以及设置 → 诊断 → 「复制诊断信息」的内容。诊断信息里不含对话内容和终端输出，路径里的用户名已替换成 `~`，但项目目录名仍可能出现，贴之前请自己看一眼。
- 界面问题请附截图或录屏。
- 安全问题不要公开提，见 [SECURITY.md](SECURITY.md)。

### 构建和测试

需要 Rust 工具链和 macOS（目前只发 macOS，Windows 计划 0.2、Linux 计划 0.3）。

```bash
# 开发运行（日常用 --fast：依赖开优化，自己的代码仍是 debug，增量编译快）
bash native/scripts/dev.sh --fast

# 测试
cargo test --manifest-path native/Cargo.toml
cargo test --manifest-path core/Cargo.toml
```

`dev.sh` 还支持 `--fake-home`、`--no-build`、`--tools=` 等参数，用法见脚本开头的注释。打包和发版流程见 [RELEASE.md](RELEASE.md)。

### 提交 PR

- PR 尽量小，一个 PR 只做一件事。
- 大改动（新功能、改架构、动交互）请先开 Issue 讨论，避免白做。
- 提交信息用 conventional commits 风格，如 `feat: ...`、`fix: ...`、`docs: ...`，中文或英文都可以。
- 提交前确认上面两条 `cargo test` 都是绿的。

### 代码约定

- 纯逻辑尽量抽成不依赖 UI 的函数 / 模块，并写单元测试；修 bug 先写能复现的测试。
- 不要在没讨论的情况下新增依赖。
- 代码注释用英文。
- 界面文案：目前代码里还是直接写的中文字符串，正在迁移到 rust-i18n 的 key（`native/locales/`，该目录目前还不存在）。新增界面文案请在 PR 或 Issue 里说明，迁移开始后一律走 key。
- 只改必须改的，匹配周围的代码风格，不顺手重构无关代码。

### 贡献的许可

除非你另外明确说明，你提交到本项目的任何贡献，都按 MIT OR Apache-2.0 双许可发布（见 [LICENSE-MIT](LICENSE-MIT)、[LICENSE-APACHE](LICENSE-APACHE)），不附加其他条款。

参与本项目即表示你同意遵守 [行为准则](CODE_OF_CONDUCT.md)。

## English

Thanks for your interest in makit. Fixes of any size are welcome.

### Filing issues

- Use the [issue templates](.github/ISSUE_TEMPLATE/) (bug report or feature request; the templates are written in Chinese, English issues are fine).
- For bugs, include the makit version (Settings → About), macOS version and chip, whether you use Claude Code or Codex, and the output of Settings → Diagnostics → "Copy diagnostics". Diagnostics contain no conversation content or terminal output, and the user name in paths is replaced with `~`, but project directory names may still appear, so please review before pasting.
- For UI problems, attach a screenshot or recording.
- Do not report security issues publicly; see [SECURITY.md](SECURITY.md).

### Build and test

You need a Rust toolchain and macOS (currently macOS only; Windows planned for 0.2, Linux for 0.3).

```bash
# run in development (--fast: dependencies optimized, your own code stays debug)
bash native/scripts/dev.sh --fast

# tests
cargo test --manifest-path native/Cargo.toml
cargo test --manifest-path core/Cargo.toml
```

See the comments at the top of `native/scripts/dev.sh` for other flags. Packaging and releasing are described in [RELEASE.md](RELEASE.md).

### Pull requests

- Keep PRs small, one concern per PR.
- Open an issue first for large changes (new features, architecture, interaction changes).
- Use conventional-commit style messages: `feat: ...`, `fix: ...`, `docs: ...`. Chinese or English are both fine.
- Make sure both `cargo test` commands above pass.

### Code conventions

- Extract pure logic into UI-independent functions or modules and unit-test it; for bug fixes, write a reproducing test first.
- No new dependencies without discussion.
- Code comments are in English.
- UI strings: currently written as Chinese literals in the code; we are migrating to rust-i18n keys (`native/locales/`, which does not exist yet). Please mention any new UI text in your PR or issue; once the migration starts, new text must use keys.
- Make surgical changes and match the surrounding style; do not refactor unrelated code.

### License of contributions

Unless you explicitly state otherwise, any contribution you submit to this project is released under the dual license MIT OR Apache-2.0 (see [LICENSE-MIT](LICENSE-MIT), [LICENSE-APACHE](LICENSE-APACHE)), without any additional terms or conditions.

By participating you agree to follow the [Code of Conduct](CODE_OF_CONDUCT.md).
