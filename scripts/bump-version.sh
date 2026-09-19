#!/bin/bash
# 同步更新四处版本号。桌面端安装包的版本来自 tauri.conf.json，
# 漏改任何一处都会导致「构建出来的包版本和 tag 对不上」。
#
# 用法: ./scripts/bump-version.sh 0.2.0
set -e
NEW="${1:?用法: $0 <版本号>  例: $0 0.2.0}"
cd "$(git rev-parse --show-toplevel)"

python3 - "$NEW" <<'PY'
import io, re, sys

new = sys.argv[1]
if not re.fullmatch(r"\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?", new):
    sys.exit("❌ 版本号格式不对: %s（需要 x.y.z，可带 -beta.1 之类后缀）" % new)

def edit(path, pattern, repl, required=True):
    try:
        s = io.open(path, encoding="utf-8").read()
    except FileNotFoundError:
        if required: sys.exit("❌ 找不到 %s" % path)
        return None
    s2, n = re.subn(pattern, repl, s, count=1)
    if n == 0:
        if required: sys.exit("❌ %s 里没匹配到版本号字段" % path)
        return None
    io.open(path, "w", encoding="utf-8").write(s2)
    return True

cur = re.search(r'"version"\s*:\s*"([^"]+)"', io.open("package.json", encoding="utf-8").read()).group(1)
print("%s → %s\n" % (cur, new))

edit("package.json",              r'("version"\s*:\s*")[^"]+"',            r'\g<1>%s"' % new)
edit("src-tauri/tauri.conf.json", r'("version"\s*:\s*")[^"]+"',            r'\g<1>%s"' % new)
edit("src-tauri/Cargo.toml",      r'(?m)^(version\s*=\s*")[^"]+"',         r'\g<1>%s"' % new)
edit("src-tauri/Cargo.lock",      r'(\[\[package\]\]\nname = "makit"\nversion = ")[^"]+"',
                                  r'\g<1>%s"' % new, required=False)

checks = [
    ("package.json",              r'"version"\s*:\s*"([^"]+)"'),
    ("src-tauri/tauri.conf.json", r'"version"\s*:\s*"([^"]+)"'),
    ("src-tauri/Cargo.toml",      r'(?m)^version\s*=\s*"([^"]+)"'),
]
print("核对：")
ok = True
for path, pat in checks:
    got = re.search(pat, io.open(path, encoding="utf-8").read()).group(1)
    flag = "✅" if got == new else "❌"
    print("   %-32s %-10s %s" % (path, got, flag))
    ok &= got == new
if not ok: sys.exit("   版本号不一致，请检查")
print("   全部一致")

print("""
接下来（master 受保护，必须走分支）：
   git checkout -b a_release_v{v}
   git commit -am "chore: 版本号 {c} → {v}"
   git checkout master && git merge --ff-only a_release_v{v}
   git push origin master
   git tag -a v{v} -m "Release v{v}" && git push origin v{v}

推 tag 触发 .github/workflows/release.yml 构建 macOS / Windows / Linux 三平台。""".format(v=new, c=cur))
PY
