#!/usr/bin/env python3
"""在假 HOME 里造一批 Claude 会话（给 sidebar 自检和 dev 里看侧栏用，#259）。

    python3 native/scripts/fake-sessions.py <假 HOME> [项目数=6] [每个项目的会话数=4]

造出来的东西：
- `<HOME>/work/projN/`：真实存在的工作目录，并且 `git init`（项目视图按 git 根归并：不是仓库的目录会被全部并进同一个组）
- `<HOME>/.claude/projects/<编码后的 cwd>/<uuid>.jsonl`：每个会话一个文件，第一条是用户消息（侧栏标题取它）
- 修改时间错开（今天 / 昨天 / 前几天 / 上周），这样侧栏的日期分段和「最近」排序都有内容
只用标准库；重复运行会先删掉旧的 `work/`、`.claude/projects/` 和上一次留下的状态文件 `.claude/makit/native-state.json`
（自检会改侧栏视图 / 折叠状态，不清掉下一次就继承）——只动假 HOME 里这几处。
"""
import os
import re
import shutil
import subprocess
import sys
import time
import uuid

home = os.path.abspath(sys.argv[1])
projects = int(sys.argv[2]) if len(sys.argv) > 2 else 6
per = int(sys.argv[3]) if len(sys.argv) > 3 else 4
if not home.startswith("/tmp/"):
    sys.exit("拒绝：假 HOME 必须在 /tmp/ 下（防止误动真实目录）：" + home)

for d in (os.path.join(home, "work"), os.path.join(home, ".claude", "projects")):
    shutil.rmtree(d, ignore_errors=True)
for f in ("native-state.json", "native-state.json.bad"):
    try:
        os.remove(os.path.join(home, ".claude", "makit", f))
    except FileNotFoundError:
        pass
now = time.time()
ages_h = [1, 3, 26, 30, 50, 80, 170, 400]  # 小时：今天、昨天、前天、上周、更早
for p in range(1, projects + 1):
    cwd = os.path.join(home, "work", f"proj{p}")
    os.makedirs(cwd, exist_ok=True)
    subprocess.run(["/usr/bin/git", "init", "-q", cwd], check=True)
    encoded = re.sub(r"[^A-Za-z0-9-]", "-", os.path.realpath(cwd))
    pdir = os.path.join(home, ".claude", "projects", encoded)
    os.makedirs(pdir, exist_ok=True)
    for s in range(per):
        sid = str(uuid.uuid4())
        path = os.path.join(pdir, sid + ".jsonl")
        with open(path, "w", encoding="utf-8") as f:
            real = os.path.realpath(cwd)
            f.write('{"type":"user","cwd":"%s","isSidechain":false,"message":{"role":"user","content":"项目%d 的第%d个会话：做点事"}}\n' % (real, p, s + 1))
            f.write('{"type":"assistant","cwd":"%s","isSidechain":false,"message":{"role":"assistant","content":[{"type":"text","text":"好的"}]}}\n' % real)
        t = now - ages_h[(p + s) % len(ages_h)] * 3600
        os.utime(path, (t, t))
print(f"造了 {projects} 个项目 × {per} 个会话 = {projects * per} 个，在 {home}")
