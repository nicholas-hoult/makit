#!/usr/bin/env bash
# 启动 GPUI 版开发模式（debug 构建 + 直接运行）。
#
#   bash native/scripts/dev.sh              # 用真实 HOME：能看到你的会话，但会抢走正在运行的 makit 的 hook
#   bash native/scripts/dev.sh --fake-home  # 用假 HOME（/tmp/mkdev）：不碰 ~/.claude，也不抢 hook，里面没有真会话
#   bash native/scripts/dev.sh --release    # 优化构建（首次全量编译很慢，用来测性能）
#   bash native/scripts/dev.sh --fake-home --tools=none|claude|codex|both
#                                           # 指定「本机装了哪些工具」（#197 空状态提示）。假 HOME 只隔离会话目录，
#                                           # 命令检测仍读真实 PATH；不加这个参数时 --fake-home 默认是 none，能看到「还没有找到」那条
#
# 注意：
# - 裸二进制没有 app 身份，系统横幅发不出来（只记通知中心 + Dock 角标）；要测横幅用 bundle.sh 打的 .app
# - 在 makit 自己的终端里跑这个，重启 dev 会杀掉里面的会话
# - 假 HOME 路径要短：hook 的 unix socket 路径有长度上限（SUN_LEN）
set -euo pipefail

# ── dev 的配置都在这里改（#242）──
# 窗口标题 / 标题栏里显示的名字，用来和线上的 makit 区分（线上不传这个变量，显示 makit）
DEV_APP_NAME="makit-dev"

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

# 放在 .worktrees/ 下的 worktree 共用同级的 .target-gpui（避免每个 worktree 各编译一份）；
# 已经设了 CARGO_TARGET_DIR 就尊重它；普通克隆用默认的 native/target
if [[ -z "${CARGO_TARGET_DIR:-}" && "$(basename "$(dirname "$root")")" == ".worktrees" ]]; then
    export CARGO_TARGET_DIR="$(dirname "$root")/.target-gpui"
fi

profile=()
fake_home=0
tools=""
for a in "$@"; do
    case "$a" in
        --fake-home) fake_home=1 ;;
        --release) profile=(--release) ;;
        --tools=none|--tools=claude|--tools=codex|--tools=both) tools="${a#--tools=}" ;;
        --tools=*) echo "--tools 的值要是 none / claude / codex / both（收到：${a#--tools=}）" >&2; exit 2 ;;
        -h|--help) awk 'NR>1 && /^#/ {print; next} NR>1 {exit}' "${BASH_SOURCE[0]}"; exit 0 ;;
        *) echo "不认识的参数：${a}（可用：--fake-home / --release / --tools=none|claude|codex|both）" >&2; exit 2 ;;
    esac
done

# ZDOTDIR 会让终端里的 shell 读到别处的配置
unset ZDOTDIR
export MAKIT_APP_NAME="${DEV_APP_NAME}"

# 先用真实 HOME 构建（cargo / rustup 要从 ~/.cargo、~/.rustup 找东西），再换 HOME 启动二进制
# macOS 自带 bash 3.2：空数组在 set -u 下展开会报 unbound，所以用 ${arr[@]+...} 的写法
build=(cargo build ${profile[@]+"${profile[@]}"} --manifest-path "$root/native/Cargo.toml")
echo "→ ${CARGO_TARGET_DIR:+CARGO_TARGET_DIR=$CARGO_TARGET_DIR }${build[*]}"
[[ "${DEV_DRY_RUN:-}" == 1 ]] || "${build[@]}"

bin="${CARGO_TARGET_DIR:-$root/native/target}/$([[ ${#profile[@]} -gt 0 ]] && echo release || echo debug)/makit-native"
if [[ $fake_home == 1 ]]; then
    export HOME="/tmp/mkdev"
    mkdir -p "${HOME}"
    # 假 HOME 默认当作「什么都没装」，这样首次打开的提示（#197）在 dev 里看得到；要看别的状态用 --tools=
    tools="${tools:-none}"
    echo "→ 假 HOME：${HOME}（不读 ~/.claude，里面没有真会话）"
fi
[[ -n "${tools}" ]] && export MAKIT_TOOLS="${tools}" && echo "→ 工具检测按 MAKIT_TOOLS=${tools} 算（不看真实 PATH）"
echo "→ ${bin}"
[[ "${DEV_DRY_RUN:-}" == 1 ]] && exit 0
exec "$bin"
