# 发版流程

0.1 只发 macOS 通用包（Intel + Apple Silicon 同一个文件），Linux 计划 0.2、Windows 计划 0.3。

## 本地打包

```bash
bash native/scripts/bundle.sh                    # 本机架构的 makit.app
UNIVERSAL=1 DMG=1 bash native/scripts/bundle.sh  # 通用包 + .dmg（要先 rustup target add aarch64-apple-darwin x86_64-apple-darwin）
```

产物在 `.worktrees/.target-gpui/release/bundle/`（没有该目录时在 `native/target/release/bundle/`）：`makit.app`、`makit-<版本>.dmg`。

签名：钥匙串里有名为 `makit-local-sign` 的自签名证书就用它（签名身份固定，重新打包后系统授权不会失效），没有就退回 ad-hoc。两种都**不是** Apple 开发者签名，别人第一次打开要手动放行（见 README）。

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

## 以后要补的

- Apple 开发者签名 + 公证（需要开发者账号，#262）
- Linux、Windows 的构建（0.2、0.3）
