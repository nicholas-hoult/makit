# 内置终端配色主题的来源与授权

本文件手工维护（不是脚本生成），对应 `native/src/theme/builtin.rs` 里的 18 套内置配色。

色值取自 GitHub 合集 [iTerm2-Color-Schemes](https://github.com/mbadolato/iTerm2-Color-Schemes)（合集本身 MIT）。**合集的 MIT 只覆盖"仓库整体"，不覆盖单个主题**——合集自己的 LICENSE 原文写明：「The copyright/license for each individual theme belongs to the author of that theme.」所以下表逐个列出每套主题的真实上游项目和授权，不是合集的 MIT。

| 主题 | 上游项目 | 授权 |
|---|---|---|
| Embark | [embark-theme/vim](https://github.com/embark-theme/vim) | MIT |
| Catppuccin Mocha / Macchiato | [catppuccin/catppuccin](https://github.com/catppuccin/catppuccin) | MIT |
| TokyoNight | [folke/tokyonight.nvim](https://github.com/folke/tokyonight.nvim) | Apache-2.0 |
| GitHub Dark Default / Light Default / Light High Contrast | [primer/github-vscode-theme](https://github.com/primer/github-vscode-theme) | MIT |
| Solarized Osaka Night | [craftzdog/solarized-osaka.nvim](https://github.com/craftzdog/solarized-osaka.nvim) | Apache-2.0 |
| Ayu Mirage | [ayu-theme/vscode-ayu](https://github.com/ayu-theme/vscode-ayu) | MIT |
| Carbonfox / Dayfox | [EdenEast/nightfox.nvim](https://github.com/EdenEast/nightfox.nvim) | MIT |
| Moonfly | [bluz71/vim-moonfly-colors](https://github.com/bluz71/vim-moonfly-colors) | MIT |
| Everblush | [Everblush/everblush](https://github.com/Everblush/everblush) | MIT |
| Zenbones Light | [mcchrish/zenbones.nvim](https://github.com/mcchrish/zenbones.nvim) | MIT |
| Nvim Light | Neovim 官方文档内置默认配色色板 | Apache-2.0 |
| Porcelain / Letterpress | [livlign/posh-palette](https://github.com/livlign/posh-palette)（PoshPalette） | MIT |
| Patina Light | [lmarkmann/patina-theme](https://github.com/lmarkmann/patina-theme) | MIT |

## 已撤下的三套（#255/#D-2114）

下面这三套曾经内置，授权核实后发现有问题，已经不再随 makit 分发；之前选用它们的用户启动时会自动换成色值最接近的现存主题，并看到一次系统提示：

- **Moonwalk**（上游 [theJian/nvim-moonwalk](https://github.com/theJian/nvim-moonwalk)）：仓库根目录没有 LICENSE 文件，README 也没写授权条款，默认视为保留所有权利、没有明确许可做二次分发
- **Modus Vivendi Tinted / Modus Operandi Tinted**（上游 [protesilaos/modus-themes](https://github.com/protesilaos/modus-themes)）：真实的 GPL-3.0-or-later 项目，作者没有发表过"色值本身不受 GPL 约束、可以自由引用"之类的声明
