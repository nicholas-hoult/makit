#!/usr/bin/env python3
"""GPUI 原型 vs Tauri 版（master）的对比测量（#221）。

必须在屏幕解锁、窗口可见时跑：屏幕锁着时 GPUI 会停掉 display link、WKWebView 会停 rAF，
两边都一帧不画，startup 记录根本写不出来。

    python3 native/scripts/bench.py --fakehome <假 HOME> --gpui native/target/release/makit-native \
        --master <master 裸二进制的拷贝> [--runs 5] [--terminal]

- master 裸二进制请先**拷贝改名**（比如 makit-baseline）再传进来：WKWebView 的数据目录按可执行文件名
  放在 ~/Library/WebKit/<名字>（不认 HOME），不改名会和正在用的 makit 共用 localStorage（#219）。
- 启动：每个应用冷启动 runs 次，读 perf.log 里的 startup 记录（从进程启动算起，到侧栏带数据画出来），
  起来 3 秒后量 RSS（master 加上它拉起的 WebKit GPU / Networking / WebContent 进程）。
- --terminal：再各启动一次，脚本在假 HOME 里放一个假的 `claude`（跑 `seq 1 300000`），
  **你在窗口里点侧栏任意一个会话**，脚本读 terminal-open 记录（点击 → 首次输出），
  并按「应用进程 CPU 时间停止增长」判定输出处理完（两边同一把尺子），GPUI 另有 terminal-burst 的帧统计。
- 只按 PID 杀自己启动的进程。
"""
import argparse
import json
import os
import re
import signal
import subprocess
import sys
import time


def pids_matching(pattern):
    out = subprocess.run(["pgrep", "-f", pattern], capture_output=True, text=True).stdout
    return {int(x) for x in out.split()}


def rss_kb(pid):
    out = subprocess.run(["ps", "-o", "rss=", "-p", str(pid)], capture_output=True, text=True).stdout.strip()
    return int(out) if out else 0


def cpu_secs(pid):
    out = subprocess.run(["ps", "-o", "time=", "-p", str(pid)], capture_output=True, text=True).stdout.strip()
    if not out:
        return 0.0
    parts = [float(x) for x in re.split("[:]", out)]
    secs = 0.0
    for p in parts:
        secs = secs * 60 + p
    return secs


def read_log(path, kind):
    try:
        with open(path) as f:
            return [json.loads(l) for l in f if f'"kind":"{kind}"' in l.replace(" ", "")]
    except FileNotFoundError:
        return []


def app_env(fakehome):
    env = {k: v for k, v in os.environ.items() if not k.startswith(("MAKIT_", "CLAUDE")) and k != "ZDOTDIR"}
    env["HOME"] = fakehome
    return env


def launch(binary, fakehome):
    webkit_before = pids_matching("com.apple.WebKit")
    p = subprocess.Popen([binary], env=app_env(fakehome), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                         cwd=fakehome)
    return p, webkit_before


def helpers(webkit_before):
    return sorted(pids_matching("com.apple.WebKit") - webkit_before)


def stop(p):
    p.send_signal(signal.SIGTERM)
    try:
        p.wait(10)
    except subprocess.TimeoutExpired:
        p.kill()
        p.wait()


def wait_for(path, kind, n, timeout):
    end = time.time() + timeout
    while time.time() < end:
        recs = read_log(path, kind)
        if len(recs) >= n:
            return recs[n - 1]
        time.sleep(0.05)
    return None


def bench_startup(name, binary, fakehome, runs):
    log = os.path.join(fakehome, ".claude", "makit", "perf.log")
    rows = []
    for i in range(runs):
        if os.path.exists(log):
            os.remove(log)
        p, before = launch(binary, fakehome)
        rec = wait_for(log, "startup", 1, 60)
        time.sleep(3)
        hs = helpers(before)
        rss_main = rss_kb(p.pid)
        rss_helpers = sum(rss_kb(h) for h in hs)
        stop(p)
        rows.append({"run": i + 1, "startup_ms": rec and rec.get("ms"), "stages": rec and rec.get("stages"),
                     "rss_main_mb": round(rss_main / 1024, 1), "rss_helpers_mb": round(rss_helpers / 1024, 1),
                     "helpers": len(hs)})
        print(f"[{name}] 启动 #{i + 1}: {rows[-1]['startup_ms']} ms, RSS 主进程 {rows[-1]['rss_main_mb']} MB"
              f" + 辅助进程 {rows[-1]['rss_helpers_mb']} MB（{len(hs)} 个）", flush=True)
        time.sleep(2)
    return rows


FAKE_CLAUDE = """#!/bin/sh
# #221 测量用的假 claude：记下开始时刻，然后大量输出
python3 -c 'import time; print(time.time())' > "$HOME/.bench-start"
seq 1 300000
"""


def bench_terminal(name, binary, fakehome):
    log = os.path.join(fakehome, ".claude", "makit", "perf.log")
    bindir = os.path.join(fakehome, "bin")
    os.makedirs(bindir, exist_ok=True)
    fake = os.path.join(bindir, "claude")
    with open(fake, "w") as f:
        f.write(FAKE_CLAUDE)
    os.chmod(fake, 0o755)
    zprofile = os.path.join(fakehome, ".zprofile")
    with open(zprofile, "w") as f:
        f.write('export PATH="$HOME/bin:$PATH"\n')
    start_file = os.path.join(fakehome, ".bench-start")
    if os.path.exists(start_file):
        os.remove(start_file)
    if os.path.exists(log):
        os.remove(log)
    p, before = launch(binary, fakehome)
    wait_for(log, "startup", 1, 60)
    print(f"[{name}] 请在窗口里点侧栏任意一个会话（120 秒内）……", flush=True)
    rec = wait_for(log, "terminal-open", 1, 120)
    # 等假 claude 开始
    end = time.time() + 30
    while not os.path.exists(start_file) and time.time() < end:
        time.sleep(0.02)
    t0 = float(open(start_file).read().strip()) if os.path.exists(start_file) else time.time()
    procs = [p.pid] + helpers(before)
    # CPU 时间 200ms 内增长不到 20ms、连续 1.5 秒 → 算处理完
    last_busy = time.time()
    prev = sum(cpu_secs(x) for x in procs)
    while time.time() - last_busy < 1.5 and time.time() - t0 < 300:
        time.sleep(0.2)
        cur = sum(cpu_secs(x) for x in procs)
        if cur - prev > 0.02:
            last_busy = time.time()
        prev = cur
    done_ms = round((last_busy - t0) * 1000)
    hs = helpers(before)
    rss = rss_kb(p.pid) / 1024
    rss_h = sum(rss_kb(h) for h in hs) / 1024
    burst = read_log(log, "terminal-burst")
    stop(p)
    os.remove(fake)
    os.remove(zprofile)
    out = {"terminal_open": rec, "burst_done_ms": done_ms, "burst_records": burst,
           "rss_main_mb": round(rss, 1), "rss_helpers_mb": round(rss_h, 1)}
    print(f"[{name}] 打开终端 {rec and rec.get('ms')} ms；seq 1 300000 处理完 {done_ms} ms；"
          f"RSS {out['rss_main_mb']} + {out['rss_helpers_mb']} MB；帧统计 {burst[-1] if burst else '无'}", flush=True)
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--fakehome", required=True)
    ap.add_argument("--gpui", required=True)
    ap.add_argument("--master", required=True)
    ap.add_argument("--runs", type=int, default=5)
    ap.add_argument("--terminal", action="store_true")
    ap.add_argument("--out", default="bench-result.json")
    a = ap.parse_args()
    fh = os.path.abspath(a.fakehome)
    result = {}
    for name, binary in (("gpui", a.gpui), ("master", a.master)):
        result[name] = {"startup": bench_startup(name, os.path.abspath(binary), fh, a.runs),
                        "binary_mb": round(os.path.getsize(binary) / 1024 / 1024, 1)}
        if a.terminal:
            result[name]["terminal"] = bench_terminal(name, os.path.abspath(binary), fh)
    with open(a.out, "w") as f:
        json.dump(result, f, ensure_ascii=False, indent=2)
    print("\n| 指标 | master | GPUI |\n|---|---|---|")

    def med(xs):
        xs = sorted(x for x in xs if x is not None)
        return xs[len(xs) // 2] if xs else None

    for label, key in (("启动到侧栏有数据（中位数 ms）", "startup_ms"), ("空闲 RSS 主进程（MB）", "rss_main_mb"),
                       ("空闲 RSS 辅助进程（MB）", "rss_helpers_mb")):
        print(f"| {label} | {med([r[key] for r in result['master']['startup']])} | "
              f"{med([r[key] for r in result['gpui']['startup']])} |")
    print(f"| 二进制大小（MB） | {result['master']['binary_mb']} | {result['gpui']['binary_mb']} |")
    print(f"\n完整结果：{os.path.abspath(a.out)}")


if __name__ == "__main__":
    sys.exit(main())
