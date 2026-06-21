# 发布指南

## 自动构建（推荐）

使用 GitHub Actions 自动构建所有平台版本：

### 1. 创建 Release Tag

```bash
# 更新版本号（package.json 和 src-tauri/tauri.conf.json）
# 然后创建并推送 tag
git tag v0.1.0
git push origin v0.1.0
```

### 2. 等待自动构建

GitHub Actions 会自动构建三个平台：
- **macOS Universal** (Intel + Apple Silicon)
- **Windows** (x64)
- **Linux** (x64)

构建产物会自动上传到 GitHub Releases（草稿状态）

### 3. 发布

1. 前往 https://github.com/yourusername/makit/releases
2. 编辑草稿 Release
3. 完善 Release Notes
4. 点击 "Publish release"

---

## 手动构建

### macOS Universal

在 macOS 上构建同时支持 Intel 和 Apple Silicon 的版本：

```bash
# 安装依赖
pnpm install

# 构建 Universal binary
pnpm tauri build --target universal-apple-darwin

# 产物位置
# src-tauri/target/universal-apple-darwin/release/bundle/macos/makit.app
```

### macOS 分平台构建

如果只想构建特定架构：

```bash
# Apple Silicon (M1/M2/M3)
pnpm tauri build --target aarch64-apple-darwin

# Intel
pnpm tauri build --target x86_64-apple-darwin
```

### Windows

在 Windows 机器上：

```bash
pnpm install
pnpm tauri build

# 产物位置
# src-tauri/target/release/bundle/msi/makit_0.1.0_x64_en-US.msi
# src-tauri/target/release/bundle/nsis/makit_0.1.0_x64-setup.exe
```

### Linux

在 Linux 机器上：

```bash
# 安装系统依赖
sudo apt-get update
sudo apt-get install -y libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf

# 构建
pnpm install
pnpm tauri build

# 产物位置
# src-tauri/target/release/bundle/deb/makit_0.1.0_amd64.deb
# src-tauri/target/release/bundle/appimage/makit_0.1.0_amd64.AppImage
```

---

## 快速脚本

### macOS 一键构建

```bash
chmod +x scripts/build-release.sh
./scripts/build-release.sh
```

---

## 签名与公证（可选）

### macOS 签名

需要 Apple Developer 账号和证书：

```bash
# 签名
codesign --force --deep --sign "Developer ID Application: Your Name (TEAM_ID)" makit.app

# 验证
codesign --verify --verbose makit.app
```

### macOS 公证

```bash
# 打包成 zip
ditto -c -k --keepParent makit.app makit.zip

# 上传公证
xcrun notarytool submit makit.zip \
  --apple-id your@email.com \
  --password "app-specific-password" \
  --team-id TEAM_ID \
  --wait

# 钉合公证凭证
xcrun stapler staple makit.app
```

### Windows 签名

需要代码签名证书。

---

## 发布检查清单

- [ ] 更新版本号（package.json, tauri.conf.json）
- [ ] 更新 CHANGELOG.md
- [ ] 测试构建产物是否正常运行
- [ ] 检查应用图标是否正确
- [ ] 准备 Release Notes
- [ ] 创建并推送 git tag
- [ ] 等待 CI 构建完成
- [ ] 检查所有平台的构建产物
- [ ] 发布 GitHub Release
- [ ] （可选）签名和公证

---

## 构建产物说明

| 平台 | 格式 | 说明 |
|-----|------|------|
| macOS | `.app` | 应用包（可直接运行） |
| macOS | `.dmg` | 磁盘镜像（推荐分发格式） |
| Windows | `.msi` | Windows Installer |
| Windows | `.exe` | NSIS 安装程序 |
| Linux | `.deb` | Debian/Ubuntu 包 |
| Linux | `.AppImage` | 通用格式（无需安装） |

---

## 故障排查

### macOS: 构建失败

```bash
# 确保安装了 Xcode Command Line Tools
xcode-select --install

# 安装 Rust targets
rustup target add aarch64-apple-darwin x86_64-apple-darwin
```

### Windows: 构建失败

确保安装了 Visual Studio Build Tools 和 WebView2。

### Linux: 构建失败

安装缺失的系统依赖（见上方 Linux 构建部分）。

---

## CI/CD 配置

GitHub Actions workflow 位于 `.github/workflows/release.yml`

触发条件：
- 推送以 `v` 开头的 tag（如 `v0.1.0`）
- 手动触发（Actions 页面 → Run workflow）

构建矩阵：
- macOS (Universal)
- Windows (x64)
- Ubuntu (x64)

所有构建产物会自动上传到 GitHub Releases。
