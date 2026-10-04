#!/usr/bin/env python3
"""生成 THIRD_PARTY_LICENSES.md（#255）：列出进最终二进制的第三方 crate 及其许可证。

    python3 native/scripts/third-party-licenses.py

只用标准库。依赖图取 makit-native 的 normal 依赖（排除 dev / build-only），目标平台 x86_64-apple-darwin。
输出按许可证、名字、版本排序，不含时间戳，同一份 Cargo.lock 下结果固定。
"""
import json
import re
import subprocess
import sys
from collections import defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "THIRD_PARTY_LICENSES.md"
ROOT_PKG = "makit-native"

# 不需要特别提示的许可证（SPDX id）
PERMISSIVE = {
    "MIT", "Apache-2.0", "Apache-2.0 WITH LLVM-exception", "BSD-2-Clause", "BSD-3-Clause",
    "ISC", "Zlib", "Unicode-DFS-2016", "Unicode-3.0", "Unlicense", "CC0-1.0", "0BSD",
    "BSL-1.0", "MIT-0", "CDLA-Permissive-2.0",
}

# 非宽松许可证的一句话义务说明；没列到的按「需要人工确认」处理
OBLIGATIONS = {
    "MPL-2.0": "文件级 copyleft：对这些文件本身的修改要以 MPL-2.0 公开；原样作为依赖使用时无额外要求。",
}


def load_metadata():
    out = subprocess.check_output(
        ["cargo", "metadata", "--format-version", "1", "--filter-platform", "x86_64-apple-darwin",
         "--manifest-path", str(ROOT / "native" / "Cargo.toml")],
        text=True,
    )
    return json.loads(out)


def shipped_packages(meta):
    pkgs = {p["id"]: p for p in meta["packages"]}
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    root_id = next(i for i, p in pkgs.items() if p["name"] == ROOT_PKG)
    seen, stack = set(), [root_id]
    while stack:
        cur = stack.pop()
        if cur in seen:
            continue
        seen.add(cur)
        for dep in nodes[cur]["deps"]:
            # dep_kinds 里 kind 为 None 表示 normal 依赖；全是 dev / build 的跳过
            if any(k["kind"] is None for k in dep["dep_kinds"]):
                stack.append(dep["pkg"])
    workspace = set(meta["workspace_members"])
    return [pkgs[i] for i in seen if i not in workspace and pkgs[i]["source"] is not None]


def license_of(pkg):
    lic = pkg.get("license")
    if lic:
        # 老 crate 用 "MIT/Apache-2.0" 的写法，统一成 SPDX 的 OR
        return re.sub(r"\s+", " ", lic.replace("/", " OR ")).strip()
    if pkg.get("license_file"):
        return "见 " + Path(pkg["license_file"]).name
    return "UNKNOWN"


def spdx_ids(expr):
    return {t for t in re.split(r"[\s()]+", expr) if t and t not in ("OR", "AND", "WITH")}


def is_permissive(expr):
    """OR 表达式里只要有一个选项全是宽松许可证就算宽松；AND 要求每项都宽松。"""
    for option in re.split(r"\s+OR\s+", expr.replace("(", "").replace(")", "")):
        terms = re.split(r"\s+AND\s+", option)
        if all(t.strip() in PERMISSIVE or spdx_ids(t) <= PERMISSIVE | {"LLVM-exception"} for t in terms):
            return True
    return False


def main():
    meta = load_metadata()
    pkgs = shipped_packages(meta)
    rows = defaultdict(list)
    for p in pkgs:
        rows[license_of(p)].append(p)

    flagged = {lic: ps for lic, ps in rows.items() if not is_permissive(lic)}

    out = []
    w = out.append
    w("# 第三方许可证\n")
    w("本文件由 `native/scripts/third-party-licenses.py` 生成，请勿手改；依赖变更后重新运行脚本。\n")
    w(f"范围：`{ROOT_PKG}` 的 normal 依赖（目标 x86_64-apple-darwin，不含 dev / build-only），共 {len(pkgs)} 个 crate；"
      "另含随应用打包的非 crate 素材。makit 自身为 MIT OR Apache-2.0。\n")

    w("## 需要留意的许可证\n")
    if flagged:
        w("下列依赖的许可证不在 MIT / Apache-2.0 / BSD / ISC / Zlib / Unicode / Unlicense / CC0 之列：\n")
        for lic in sorted(flagged):
            names = ", ".join(f"{p['name']} {p['version']}" for p in sorted(flagged[lic], key=lambda p: (p["name"], p["version"])))
            ob = OBLIGATIONS.get(lic, "需要人工确认条款。")
            w(f"- `{lic}`：{ob} 涉及：{names}")
        w("")
    else:
        w("无。\n")

    w("## 许可证文本在哪里\n")
    for lic in sorted(rows):
        ids = sorted(spdx_ids(lic) - {"LLVM-exception"})
        urls = ", ".join(f"<https://spdx.org/licenses/{i}.html>" for i in ids)
        w(f"- `{lic}`：各 crate 源码包内的 LICENSE 文件（crates.io 上对应版本的源码），标准文本见 {urls}")
    w("")

    w("## Rust 依赖\n")
    for lic in sorted(rows):
        w(f"### {lic}（{len(rows[lic])}）\n")
        w("| 名称 | 版本 | 许可证 | 仓库 |")
        w("|---|---|---|---|")
        for p in sorted(rows[lic], key=lambda p: (p["name"], p["version"])):
            repo = p.get("repository") or p.get("homepage") or ""
            w(f"| {p['name']} | {p['version']} | {lic} | {repo} |")
        w("")

    w("## 随应用打包的其他素材\n")
    w("| 素材 | 位置 | 许可证 | 说明 |")
    w("|---|---|---|---|")
    w("| JetBrains Mono 字体 | `native/assets/fonts/*.ttf` | SIL OFL 1.1 | 文本见 `native/assets/fonts/OFL.txt`；版权 2020 The JetBrains Mono Project Authors，<https://github.com/JetBrains/JetBrainsMono> |")
    w("| 内置终端配色（21 套） | `native/src/theme/builtin.rs` | MIT（合集） | 取自 iTerm2-Color-Schemes 合集 <https://github.com/mbadolato/iTerm2-Color-Schemes>（MIT）。合集本身是 MIT，但单个主题原作者的授权未逐个核实 |")
    w("| 应用图标 | `src-tauri/icons/icon.icns` | 待确认 | provenance to be confirmed：仓库内没有记录来源 |")
    w("| 工具图标（Claude / Codex 的 .ico） | `native/assets/logos/` | 待确认 | provenance to be confirmed：为对应厂商的商标 / 品牌标识，仅用于指示会话所属工具 |")
    w("| 界面 SVG 图标 | `native/assets/icons/*.svg` | 随 makit（MIT OR Apache-2.0） | 从本项目早期前端抽出；来源见各文件头注释，如有外部来源待确认 |")
    w("")

    OUT.write_text("\n".join(out), encoding="utf-8")
    print(f"写入 {OUT}（{len(pkgs)} 个 crate，{len(flagged)} 类需留意的许可证）")
    return 0


if __name__ == "__main__":
    sys.exit(main())
