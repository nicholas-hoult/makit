# 发版流程

0.1 只发 macOS 通用包（Intel + Apple Silicon 同一个文件），Windows 计划 0.2、Linux 计划 0.3。

## 本地打包

```bash
bash native/scripts/bundle.sh                    # 本机架构的 makit.app
UNIVERSAL=1 DMG=1 bash native/scripts/bundle.sh  # 通用包 + .dmg（要先 rustup target add aarch64-apple-darwin x86_64-apple-darwin）
```

产物在 `.worktrees/.target-gpui/release/bundle/`（没有该目录时在 `native/target/release/bundle/`）：`makit.app`、`makit-<版本>-universal.dmg`（带 `ARCH=aarch64|x86_64` 时是 `-arm64` / `-x64`）。

本地构建的签名：钥匙串里有名为 `makit-local-sign` 的自签名证书就用它（签名身份固定，重新打包后系统授权不会失效），没有就退回 ad-hoc。两种都**不是** Apple 开发者签名，只够自己的机器用。

发布包要用 Developer ID 签名并公证，见下面「签名与公证」。

## 签名与公证

发布包用 `Developer ID Application` 证书签名、开 hardened runtime、交 Apple 公证并把票据钉在 .dmg 上，用户双击就能打开，不用手动放行（#262）。CI 在推 `v*` tag 时自动做，**缺 Secrets 会直接失败**，不会发出没签名的包。

### 一次性设置

1. **证书**：在 [Apple Developer](https://developer.apple.com/account/resources/certificates) 创建 `Developer ID Application` 证书（只有账号持有者能建；要先用钥匙串访问生成 CSR）。装到钥匙串后，在「钥匙串访问」里连私钥一起导出成 `.p12`，设一个密码。证书全名形如 `Developer ID Application: <姓名> (<团队 ID>)`，**会公开显示在每个用户看到的签名信息里**。
2. **公证密钥**：App Store Connect >「用户和访问」>「集成」>「团队密钥」，建一个角色为 Developer 的 API key，下载 `.p8`（只能下载一次），记下 Key ID 和 Issuer ID。
3. **Secrets**：在 GitHub 仓库 Settings > Secrets and variables > Actions 里加六个：

   | Secret | 内容 |
   |---|---|
   | `APPLE_CERTIFICATE_BASE64` | `base64 -i cert.p12` 的输出 |
   | `APPLE_CERTIFICATE_PASSWORD` | `.p12` 的密码 |
   | `APPLE_SIGNING_IDENTITY` | 证书全名（上面那串） |
   | `ASC_API_KEY_ID` | API key 的 Key ID |
   | `ASC_API_ISSUER_ID` | Issuer ID |
   | `ASC_API_KEY_P8_BASE64` | `base64 -i AuthKey_XXXX.p8` 的输出 |

   密钥和证书不进仓库、不贴到聊天里。

### 本地手动签名 + 公证

```bash
SIGN_IDENTITY="Developer ID Application: ..." NOTARIZE=1 \
NOTARY_KEY_PATH=~/AuthKey_XXXX.p8 NOTARY_KEY_ID=... NOTARY_ISSUER_ID=... \
UNIVERSAL=1 DMG=1 bash native/scripts/bundle.sh
```

只设 `SIGN_IDENTITY` 不设 `NOTARIZE`：签名但不公证（调试签名用）。`NOTARIZE=1` 没有 `SIGN_IDENTITY` 会直接报错。公证通常几分钟，脚本会等结果；被拒时会打出 Apple 给的原因。

### 验证

```bash
xcrun stapler validate makit-<版本>-universal.dmg
spctl -a -t open --context context:primary-signature -vv makit-<版本>-universal.dmg   # accepted，来源 Notarized Developer ID
```

CI 的「Verify signing and notarization」步骤对每个 .dmg 做同样的检查，不通过就不会发布。

### 权限声明

`native/packaging/makit.entitlements` 声明麦克风、摄像头、通讯录、日历、位置、照片、Apple Events。makit 自己不用，但终端里跑的程序会用，系统把弹窗算在宿主 app 头上：hardened runtime 下缺了这些声明，请求会被直接拒绝而不是弹窗。对应的说明文案在 `bundle.sh` 生成的 `Info.plist` 里。

## 发布到 GitHub

1. 改版本号：`native/Cargo.toml`、`core/Cargo.toml`（两处保持一致），更新 `CHANGELOG.md`（英文，Release 说明从它生成）和 `CHANGELOG.zh-CN.md`（中文，CI 会附在英文后面）
2. 跑测试：`cargo test --manifest-path native/Cargo.toml`
3. 提交、打 tag 并推送：

   ```bash
   git tag v0.1.1
   git push origin v0.1.1
   ```

4. `.github/workflows/release.yml` 会在 macOS 上跑测试、打通用包和 .dmg，测试通过后**直接发布** Release（说明取自 CHANGELOG；测试或打包失败不会发布）
5. 发布后在 Releases 页下载 .dmg 装一遍确认；有问题可以在页面上编辑说明或设为预发布

第一次建议先在 Actions 页用 `workflow_dispatch` 手动试跑，确认构建能过，再打 tag。

## Homebrew tap

`brew install --cask nicholas-hoult/tap/makit`（#268）。发版 workflow 的最后一步会把 `packaging/homebrew/makit.rb.in` 渲染成 `Casks/makit.rb`（填入版本号和 dmg 的 sha256）并推到 `nicholas-hoult/homebrew-tap`。

一次性设置：在 GitHub 创建一个只授权 `homebrew-tap` 仓库、权限 Contents 读写的 fine-grained token，存进 makit 仓库的 Actions secret `HOMEBREW_TAP_TOKEN`。没设这个 secret 时那一步会跳过，不影响发版。

## 以后要补的

- Windows、Linux 的构建（0.2、0.3）
