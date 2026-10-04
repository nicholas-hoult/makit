#!/usr/bin/env bash
# 把 GPUI 版打成可双击的 .app（#232）。cargo 产出的是裸二进制，双击打不开、
# Dock 里没图标，更要紧的是**拿不到通知授权** —— UNUserNotificationCenter 要求
# 调用方在一个有 bundle id 的 .app 里，裸二进制发通知会被系统静默丢弃。
#
# 名字 / id / 图标都在 bundle.conf 里，这个脚本不该有需要改的地方。
#
#   bash native/scripts/bundle.sh          # release 构建并打包
#   bash native/scripts/bundle.sh --open   # 打包完顺手启动
#   UNIVERSAL=1 bash native/scripts/bundle.sh   # Intel + Apple Silicon 通用包（要先 rustup target add 两个目标）
#   DMG=1 bash native/scripts/bundle.sh         # 再生成 .dmg 安装镜像
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
# shellcheck source=../bundle.conf
source "$root/native/bundle.conf"
version="$(grep -m1 '^version' "$root/native/Cargo.toml" | cut -d'"' -f2)"

# 编译目录：已经设了 CARGO_TARGET_DIR 就尊重它；否则 ①在 .worktrees/ 下的 worktree，或 ②主目录且有 .worktrees/.target-gpui，
# 都用这个共享目录（避免每个 worktree 各编译一份；也避免「在主目录跑就用另一个目录、整套依赖从头编」）；都不是才用默认的 native/target
if [[ -z "${CARGO_TARGET_DIR:-}" ]]; then
    if [[ "$(basename "$(dirname "$root")")" == ".worktrees" ]]; then
        export CARGO_TARGET_DIR="$(dirname "$root")/.target-gpui"
    elif [[ -d "$root/.worktrees/.target-gpui" ]]; then
        export CARGO_TARGET_DIR="$root/.worktrees/.target-gpui"
    fi
fi

target="${CARGO_TARGET_DIR:-$root/native/target}"
if [[ "${UNIVERSAL:-}" == "1" ]]; then
    # 通用包（Intel + Apple Silicon 同一个文件，#191）：两个目标各编一次，lipo 合成一个二进制放回 release/ 下，后面照常打包
    for t in aarch64-apple-darwin x86_64-apple-darwin; do
        cargo build --release --target "$t" --manifest-path "$root/native/Cargo.toml"
    done
    mkdir -p "$target/release"
    lipo -create -output "$target/release/makit-native" \
        "$target/aarch64-apple-darwin/release/makit-native" "$target/x86_64-apple-darwin/release/makit-native"
else
    cargo build --release --manifest-path "$root/native/Cargo.toml"
fi

app="$target/release/bundle/$APP_NAME.app"
# 整个删掉重建：留着旧的会让上一次的残留文件（改名前的图标、删掉的资源）混在里面，
# 而 codesign 会把它们一起签进去，之后排查起来看不出是陈的。
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"

cp "$target/release/makit-native" "$app/Contents/MacOS/$EXECUTABLE_NAME"
cp "$root/$ICON_PATH" "$app/Contents/Resources/AppIcon.icns"

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
        <key>CFBundleName</key><string>$APP_NAME</string>
        <key>CFBundleDisplayName</key><string>$APP_NAME</string>
        <key>CFBundleIdentifier</key><string>$BUNDLE_ID</string>
        <key>CFBundleExecutable</key><string>$EXECUTABLE_NAME</string>
        <key>CFBundleIconFile</key><string>AppIcon</string>
        <key>CFBundlePackageType</key><string>APPL</string>
        <key>CFBundleShortVersionString</key><string>$version</string>
        <key>CFBundleVersion</key><string>$version</string>
        <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
        <key>LSMinimumSystemVersion</key><string>10.15</string>
        <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST

# 本机运行够用，给别人装要换成开发者证书 + 公证。不签的话通知授权、键盘监听这类权限申请会被系统拒掉。
# 优先用钥匙串里的本地自签名证书 makit-local-sign：签名身份固定，重新打包后录屏等授权不会失效；
# ad-hoc（-s -）每次编译哈希都变，系统会把已有授权判成另一个程序。
sign_id="-"
if security find-identity -p codesigning | grep -q '"makit-local-sign"'; then
    sign_id="makit-local-sign"
fi
codesign --force --sign "$sign_id" "$app"
codesign -dv "$app" 2>&1 | grep -E 'Identifier|Signature|Authority' || true

echo "→ $app"
if [[ "${DMG:-}" == "1" ]]; then
    # 拖进「应用程序」的安装镜像；版本号进文件名，release 里一眼分得出
    dmg="$target/release/bundle/$APP_NAME-$version.dmg"
    rm -f "$dmg"
    # 镜像里放 .app 和「应用程序」的快捷方式，打开后直接拖过去
    stage="$(mktemp -d)"
    cp -R "$app" "$stage/"
    ln -s /Applications "$stage/Applications"
    hdiutil create -quiet -volname "$APP_NAME" -srcfolder "$stage" -ov -format UDZO "$dmg"
    rm -rf "$stage"
    echo "→ $dmg"
fi
if [[ "${1:-}" == "--open" ]]; then open "$app"; fi
