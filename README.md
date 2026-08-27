# DS5Dongle BL618 OTA

这个仓库只保存 DS5Dongle OTA 相关内容，不保存上游完整源码。

- 上游仓库：<https://github.com/sqlCRT/ds5dongle-bl618-opensource>
- 本仓库：<https://github.com/mainlxl/ds5dongle-bl618-ota>
- OTA 补丁：`patches/ota/`
- 补丁应用脚本：`tools/apply-ota-patches.sh`
- 桌面 OTA 工具：`tools/ota-updater/`
- Cloudflare Web OTA：`web-ota/`

## 自动发布

`.github/workflows/build-ota-release.yml` 每小时检查一次上游最新 Release。

如果本仓库还没有同名 Release，Action 会：

1. 拉取上游 Release tag 对应源码。
2. 应用 `patches/ota/` 中的 OTA 补丁。
3. 构建 LCTech BL616 的 Full-Speed / High-Speed 固件。
4. 发布同名 Release，附件包含 `.bin`、`.bin.ota`、`.xz.ota`、DevCube 刷机 zip 和 SHA256 校验文件。

补丁应用或 OTA 接入校验失败时不会发布固件，避免把 OTA 能力刷没。

## 本地应用补丁

```bash
bash tools/apply-ota-patches.sh /path/to/upstream-src patches/ota
```

如果脚本输出 `OTA integration verified.`，说明上游源码已经接入 OTA。

## 桌面 OTA 工具

```bash
cargo run --manifest-path tools/ota-updater/Cargo.toml
```

macOS 本地打包：

```bash
cargo build --release --manifest-path tools/ota-updater/Cargo.toml
tools/ota-updater/package-macos.sh
```

## Web OTA

`web-ota/` 是 Cloudflare Pages 静态版 WebHID OTA 页面。固件同步 Action 会把 `.bin.ota` 和 SHA256 文件上传到 Cloudflare，并生成 `manifest.json`。网页运行时只读取同源静态文件，不调用 GitHub API，也不代理 GitHub Release 附件。
