#!/usr/bin/env bash
# 把 GPUI 版打成可双击的 .app（#232）。cargo 产出的是裸二进制，双击打不开、
# Dock 里没图标，更要紧的是**拿不到通知授权** —— UNUserNotificationCenter 要求
# 调用方在一个有 bundle id 的 .app 里，裸二进制发通知会被系统静默丢弃。
#
# 名字 / id / 图标都在 bundle.conf 里，这个脚本不该有需要改的地方。
#
#   bash native/scripts/bundle.sh          # release 构建并打包
#   bash native/scripts/bundle.sh --open   # 打包完顺手启动
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
# shellcheck source=../bundle.conf
source "$root/native/bundle.conf"
version="$(grep -m1 '^version' "$root/native/Cargo.toml" | cut -d'"' -f2)"

cargo build --release --manifest-path "$root/native/Cargo.toml"

# 认 CARGO_TARGET_DIR（gpui-main 的 worktree 共用一个 target 目录）
target="${CARGO_TARGET_DIR:-$root/native/target}"
app="$target/release/bundle/$APP_NAME.app"
# 整个删掉重建：留着旧的会让上一次的残留文件（改名前的图标、删掉的资源）混在里面，
# 而 codesign 会把它们一起签进去，之后排查起来看不出是陈的。
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"

cp "$target/release/makit-native" "$app/Contents/MacOS/"
cp "$root/$ICON_PATH" "$app/Contents/Resources/AppIcon.icns"

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
        <key>CFBundleName</key><string>$APP_NAME</string>
        <key>CFBundleDisplayName</key><string>$APP_NAME</string>
        <key>CFBundleIdentifier</key><string>$BUNDLE_ID</string>
        <key>CFBundleExecutable</key><string>makit-native</string>
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

# ad-hoc 签名（-s -）：本机运行够用，给别人装要换成开发者证书 + 公证。
# 不签的话通知授权、键盘监听这类权限申请会被系统拒掉。
codesign --force --sign - "$app"
codesign -dv "$app" 2>&1 | grep -E 'Identifier|Signature' || true

echo "→ $app"
[[ "${1:-}" == "--open" ]] && open "$app"
