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
#   ARCH=aarch64 DMG=1 bash native/scripts/bundle.sh   # 单架构包（aarch64 或 x86_64）；.dmg 名字带 -arm64 / -x64
#   DMG=1 bash native/scripts/bundle.sh         # 再生成 .dmg 安装镜像（通用包名字带 -universal）
#   SIGN_IDENTITY="Developer ID Application: ..." NOTARIZE=1 DMG=1 bash native/scripts/bundle.sh   # 发布包：签名 + 公证（见 RELEASE.md）
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
dmg_suffix=""
if [[ -n "${ARCH:-}" ]]; then
    # 单架构包：只编这一个目标，二进制放回 release/ 下，后面照常打包
    case "$ARCH" in
        aarch64) dmg_suffix="-arm64" ;;
        x86_64) dmg_suffix="-x64" ;;
        *) echo "ARCH must be aarch64 or x86_64" >&2; exit 1 ;;
    esac
    cargo build --release --target "$ARCH-apple-darwin" --manifest-path "$root/native/Cargo.toml"
    mkdir -p "$target/release"
    cp "$target/$ARCH-apple-darwin/release/makit-native" "$target/release/makit-native"
elif [[ "${UNIVERSAL:-}" == "1" ]]; then
    dmg_suffix="-universal"
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
cp "$root/THIRD_PARTY_LICENSES.md" "$app/Contents/Resources/THIRD_PARTY_LICENSES.md"

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
        <!-- Shown in the system permission prompts. They are raised by programs running inside the terminal, and macOS asks in the name of the hosting app -->
        <key>NSMicrophoneUsageDescription</key><string>A program running in a makit terminal is asking for access to the microphone.</string>
        <key>NSCameraUsageDescription</key><string>A program running in a makit terminal is asking for access to the camera.</string>
        <key>NSAppleEventsUsageDescription</key><string>A program running in a makit terminal is asking for access to other apps (Apple Events).</string>
        <key>NSContactsUsageDescription</key><string>A program running in a makit terminal is asking for access to your contacts.</string>
        <key>NSCalendarsUsageDescription</key><string>A program running in a makit terminal is asking for access to your calendars.</string>
        <key>NSLocationUsageDescription</key><string>A program running in a makit terminal is asking for access to your location.</string>
        <key>NSPhotoLibraryUsageDescription</key><string>A program running in a makit terminal is asking for access to your photos.</string>
        <key>NSDesktopFolderUsageDescription</key><string>A program running in a makit terminal is asking for access to your Desktop folder.</string>
        <key>NSDocumentsFolderUsageDescription</key><string>A program running in a makit terminal is asking for access to your Documents folder.</string>
        <key>NSDownloadsFolderUsageDescription</key><string>A program running in a makit terminal is asking for access to your Downloads folder.</string>
        <key>NSRemovableVolumesUsageDescription</key><string>A program running in a makit terminal is asking for access to removable volumes.</string>
        <key>NSNetworkVolumesUsageDescription</key><string>A program running in a makit terminal is asking for access to network volumes.</string>
</dict>
</plist>
PLIST

# Distribution signing: SIGN_IDENTITY names a "Developer ID Application" certificate in the keychain. That turns on the
# hardened runtime and a secure timestamp (both required for notarization) and the entitlements the programs running
# inside the terminal need (packaging/makit.entitlements). Without it (a local build) the app is signed with the local
# self-signed certificate makit-local-sign if there is one, else ad hoc: enough to run on this machine, and the system
# permission grants (screen recording and the like) survive a rebuild only with a stable identity; an ad-hoc hash changes
# on every compile and the system takes the new build for another program.
if [[ -n "${SIGN_IDENTITY:-}" ]]; then
    codesign --force --timestamp --options runtime --entitlements "$root/native/packaging/makit.entitlements" --sign "$SIGN_IDENTITY" "$app"
    codesign --verify --strict --verbose=2 "$app"
else
    sign_id="-"
    if security find-identity -p codesigning | grep -q '"makit-local-sign"'; then
        sign_id="makit-local-sign"
    fi
    codesign --force --sign "$sign_id" "$app"
fi
codesign -dv "$app" 2>&1 | grep -E 'Identifier|Signature|Authority|flags' || true

echo "→ $app"
if [[ "${DMG:-}" == "1" ]]; then
    # Notarize and staple the .app before it goes into the .dmg, so the ticket travels with the app after it is dragged out
    if [[ -n "${SIGN_IDENTITY:-}" && "${NOTARIZE:-}" == "1" ]]; then bash "$root/native/scripts/notarize.sh" "$app"; fi
    # 拖进「应用程序」的安装镜像；版本号进文件名，release 里一眼分得出
    dmg="$target/release/bundle/$APP_NAME-$version$dmg_suffix.dmg"
    rm -f "$dmg"
    # 镜像里放 .app 和「应用程序」的快捷方式，打开后直接拖过去
    stage="$(mktemp -d)"
    cp -R "$app" "$stage/"
    ln -s /Applications "$stage/Applications"
    hdiutil create -quiet -volname "$APP_NAME" -srcfolder "$stage" -ov -format UDZO "$dmg"
    rm -rf "$stage"
    if [[ -n "${SIGN_IDENTITY:-}" ]]; then
        codesign --force --timestamp --sign "$SIGN_IDENTITY" "$dmg"
        codesign --verify --verbose=2 "$dmg"
        # NOTARIZE=1 also needs NOTARY_KEY_PATH / NOTARY_KEY_ID / NOTARY_ISSUER_ID, see notarize.sh
        if [[ "${NOTARIZE:-}" == "1" ]]; then bash "$root/native/scripts/notarize.sh" "$dmg"; fi
    elif [[ "${NOTARIZE:-}" == "1" ]]; then
        echo "NOTARIZE=1 needs SIGN_IDENTITY: an unsigned build cannot be notarized" >&2
        exit 1
    fi
    echo "→ $dmg"
fi
if [[ "${1:-}" == "--open" ]]; then open "$app"; fi
