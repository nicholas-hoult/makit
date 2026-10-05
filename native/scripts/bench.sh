#!/usr/bin/env bash
# Startup / memory benchmark for a packaged makit (#264). Prints a Markdown table.
#
#   bash native/scripts/bench.sh [path/to/makit]          # default: the .app bundle built by bundle.sh
#   RUNS=5 bash native/scripts/bench.sh
#
# Each scenario builds a throw-away fake HOME under /tmp (fake-sessions.py), starts makit with HOME pointing at it,
# waits for makit's own "session list returned" startup mark, lets it settle for
# 8 s and reads the resident memory. Warm start and memory are "median (min-max)" over the RUNS-1 / RUNS runs; the first run of a scenario starts without a scan
# cache ("cold"), the following ones with the cache makit wrote ("warm").
#
# Keep the machine otherwise idle while this runs, and don't trust a number measured while something else is compiling.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
bin="${1:-}"
if [[ -z "$bin" ]]; then
    for d in "$root/.worktrees/.target-gpui" "$root/native/target"; do
        [[ -x "$d/release/bundle/makit.app/Contents/MacOS/makit" ]] && bin="$d/release/bundle/makit.app/Contents/MacOS/makit" && break
    done
fi
[[ -x "$bin" ]] || { echo "makit binary not found, pass it as the first argument" >&2; exit 1; }
runs="${RUNS:-5}"
home=/tmp/mkbench

# "median (min-max)" of the given numbers
# makit can still be flushing files for a moment after it was killed: retry the removal instead of failing the whole run
clean_home() { rm -rf "$home" 2>/dev/null || { sleep 3; rm -rf "$home"; }; }

median() { python3 -c "import sys,statistics; v=[float(x) for x in sys.argv[1:]]; print('%d (%d-%d)' % (statistics.median(v), min(v), max(v)))" "$@"; }

# one launch: prints "<sessions_loaded_ms> <rss_mb> <tree_rss_mb>"
# The time is makit's own startup mark "session list returned" (milliseconds since the process started, printed to the log
# with MAKIT_TIMING=1): the moment the session list is ready for the sidebar. It does not need a repaint, so the benchmark also
# works while the display is asleep.
launch() {
    rm -f "$home"/.claude/makit/logs/*.log
    HOME="$home" MAKIT_TOOLS=none MAKIT_TIMING=1 "$bin" >/dev/null 2>&1 &
    local pid=$!
    for _ in $(seq 1 120); do grep -qs "session list returned" "$home"/.claude/makit/logs/*.log && break; sleep 0.5; done
    sleep 8
    local rss tree ms
    rss=$(ps -o rss= -p "$pid" | tr -d ' ')
    tree=$(ps -A -o pid=,ppid=,rss= | python3 -c "
import sys
rows=[tuple(map(int,l.split())) for l in sys.stdin if l.strip()]
kids={}
for p,pp,r in rows: kids.setdefault(pp,[]).append((p,r))
rss={p:r for p,pp,r in rows}
def total(p): return rss.get(p,0)+sum(total(c) for c,_ in kids.get(p,[]))
print(total($pid))")
    ms=$(grep -hs "session list returned" "$home"/.claude/makit/logs/*.log | tail -1 | sed -E 's/.*session list returned ([0-9]+)ms.*/\1/')
    [[ "$ms" =~ ^[0-9]+$ ]] || ms=-1
    kill "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true
    echo "$ms $((rss / 1024)) $((tree / 1024))"
}

# scenario <label> <projects> <sessions per project> <kb per session> [panes]
# ONLY="regex" runs just the scenarios whose label matches (handy after a failed run)
scenario() {
    local label="$1" projects="$2" per="$3" kb="$4" tabs="${5:-1}"
    [[ -z "${ONLY:-}" || "$label" =~ $ONLY ]] || return 0
    clean_home; mkdir -p "$home"
    python3 "$root/native/scripts/fake-sessions.py" "$home" "$projects" "$per" en "$kb" >/dev/null
    mkdir -p "$home/.claude/makit"
    if [[ "$tabs" -gt 1 ]]; then
        # `tabs` panes in a balanced split tree, one shell tab each (only terminals that are shown start a shell)
        python3 - "$home" "$tabs" <<'PY'
import json,sys
home,n=sys.argv[1],int(sys.argv[2])
def pane(i):
    t={"id":f"t_{i}","kind":"shell","cwd":home,"initCommand":None,"sessionId":None,"sessionShortId":None,"label":f"shell {i}"}
    return {"kind":"container","id":f"c_{i}","tabs":[t],"activeTabId":t["id"],"tabHistory":[]}
def tree(lo,hi,vertical):
    if hi-lo==1: return pane(lo)
    mid=(lo+hi)//2
    return {"kind":"split","dir":"v" if vertical else "h","ratio":0.5,"a":tree(lo,mid,not vertical),"b":tree(mid,hi,not vertical)}
json.dump({"workspace":{"root":tree(0,n,True),"activeContainerId":"c_0"}},open(home+"/.claude/makit/native-state.json","w"))
PY
    fi
    local sessions=$((projects * per)) cold_ms warm_ms=() rss=() tree=()
    read -r cold_ms r t < <(launch); rss+=("$r"); tree+=("$t")
    for _ in $(seq 2 "$runs"); do
        read -r m r t < <(launch)
        # a run where makit never reported its startup is dropped instead of poisoning the median
        if [[ "$m" -ge 0 ]]; then warm_ms+=("$m"); rss+=("$r"); tree+=("$t"); fi
    done
    local total_mb
    total_mb=$(du -sm "$home/.claude/projects" | cut -f1)
    echo "| $label | $sessions | ${total_mb} MB | $cold_ms ms | $(median "${warm_ms[@]:-0}") ms | $(median "${rss[@]}") MB | $(median "${tree[@]}") MB |"
}

# Warm-up: the very first launch of a new binary is slower (system checks, shader compilation); keep it out of the numbers
clean_home; mkdir -p "$home"; python3 "$root/native/scripts/fake-sessions.py" "$home" 2 2 en >/dev/null; mkdir -p "$home/.claude/makit"; launch >/dev/null

echo "| Scenario | Sessions | Data on disk | Cold: session list ready | Warm: session list ready | makit RSS | makit + shells RSS |"
echo "|---|---|---|---|---|---|---|"
scenario "small sessions"            6   4    0
scenario "600 small sessions"       30  20    0
scenario "1,200 small sessions"     60  20    0
scenario "5,000 small sessions"    100  50    0
scenario "200 sessions x 2 MB"      20  10 2048
scenario "1,000 sessions x 1 MB"    50  20 1024
scenario "8 terminal panes"          6   4    0 8
clean_home
