#!/bin/bash
# 同步更新版本号：core/Cargo.toml、native/Cargo.toml、native/Cargo.lock 里的 makit-core / makit-native。
# 漏改任何一处都会导致「构建出来的包版本和 tag 对不上」。CHANGELOG 仍需手写。
#
# 用法: ./scripts/bump-version.sh 0.2.3
set -e
NEW="${1:?用法: $0 <版本号>  例: $0 0.2.3}"
cd "$(git rev-parse --show-toplevel)"

python3 - "$NEW" <<'PY'
import io, re, sys

new = sys.argv[1]
if not re.fullmatch(r"\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?", new):
    sys.exit("❌ 版本号格式不对: %s（需要 x.y.z，可带 -beta.1 之类后缀）" % new)

def edit(path, pattern, repl):
    s = io.open(path, encoding="utf-8").read()
    s2, n = re.subn(pattern, repl, s, count=1)
    if n == 0:
        sys.exit("❌ %s 里没匹配到版本号字段" % path)
    io.open(path, "w", encoding="utf-8").write(s2)

def read(path, pattern):
    return re.search(pattern, io.open(path, encoding="utf-8").read()).group(1)

toml = r'(?m)^version\s*=\s*"([^"]+)"'
lock = lambda name: r'(\[\[package\]\]\nname = "%s"\nversion = ")([^"]+)"' % name
cur = read("native/Cargo.toml", toml)
print("%s → %s\n" % (cur, new))

edit("core/Cargo.toml",   r'(?m)^(version\s*=\s*")[^"]+"', r'\g<1>%s"' % new)
edit("native/Cargo.toml", r'(?m)^(version\s*=\s*")[^"]+"', r'\g<1>%s"' % new)
for name in ("makit-core", "makit-native"):
    edit("native/Cargo.lock", lock(name), r'\g<1>%s"' % new)

checks = [
    ("core/Cargo.toml", toml),
    ("native/Cargo.toml", toml),
    ("native/Cargo.lock (makit-core)", lock("makit-core")),
    ("native/Cargo.lock (makit-native)", lock("makit-native")),
]
print("核对：")
ok = True
for path, pat in checks:
    f = path.split(" ")[0]
    m = re.search(pat, io.open(f, encoding="utf-8").read())
    got = m.group(m.lastindex)
    flag = "✅" if got == new else "❌"
    print("   %-34s %-10s %s" % (path, got, flag))
    ok &= got == new
if not ok: sys.exit("   版本号不一致，请检查")
print("   全部一致")

print("""
接下来（master 受保护，必须走分支）：
   git checkout -b release-v{v}
   # 补 CHANGELOG.md / CHANGELOG.zh-CN.md
   git commit -am "chore: release {v}"
   git checkout master && git merge --ff-only release-v{v}
   git push origin master
   git tag -a v{v} -m "Release v{v}" && git push origin v{v}

推 tag 触发 .github/workflows/release.yml。""".format(v=new))
PY
