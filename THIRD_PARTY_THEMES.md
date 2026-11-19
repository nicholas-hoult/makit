# 内置终端配色主题的来源与授权

本文件手工维护（不是脚本生成），对应 `native/src/theme/builtin.rs` 里的 21 套内置配色。

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
| Modus Vivendi Tinted / Modus Operandi Tinted | [protesilaos/modus-themes](https://github.com/protesilaos/modus-themes) | GPL-3.0-or-later（⚠️ 见下方说明） |
| Zenbones Light | [mcchrish/zenbones.nvim](https://github.com/mcchrish/zenbones.nvim) | MIT |
| Nvim Light | Neovim 官方文档内置默认配色色板 | Apache-2.0 |
| Porcelain / Letterpress | [livlign/posh-palette](https://github.com/livlign/posh-palette)（PoshPalette） | MIT |
| Patina Light | [lmarkmann/patina-theme](https://github.com/lmarkmann/patina-theme) | MIT |
| Moonwalk | [theJian/nvim-moonwalk](https://github.com/theJian/nvim-moonwalk) | ⚠️ **仓库没有 LICENSE 文件，没有授权声明——见下方说明** |

## 两个尚未解决的风险

- **Modus Vivendi/Operandi Tinted（GPL-3.0-or-later）**：上游是真实的 GPL-3.0-or-later 项目，作者没有发表过"色值本身不受 GPL 约束、可以自由引用"之类的声明。单纯引用色值数字是否真的触发 GPL 的传染条款存在争议，但这不是 makit 项目自己能判定的事。
- **Moonwalk（无 LICENSE）**：上游仓库存在，但根目录没有 LICENSE 文件、README 也没写授权条款，默认视为保留所有权利、没有明确许可做二次分发。

这两条都还没决定怎么处理（联系原作者确认、换掉、还是重新设计不声称移植），见 #255。
