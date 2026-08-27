# DS5Dongle OTA Updater

这是一个跨平台桌面 OTA 工具，用 Rust + egui + hidapi 实现，不依赖浏览器 WebHID。

## 功能

- 连接 DS5Dongle HID 设备并读取固件版本。
- 通过 GitHub `releases/latest` 跳转解析最新 tag，不调用 GitHub API。
- 按 `mainlxl/ds5dongle-bl618-ota` 的 Release 命名规则下载 `.bin.ota` 和 SHA256 校验文件。
- 通过 OTA Feature Report 写入固件，并显示接收、落盘、校验和错误状态。
- 也可以填写本地 `.bin.ota` 路径进行测试。
- 启动时加载系统中文字体，避免 egui 默认字体无法显示中文。

## 下载运行

- macOS：下载 `ds5dongle-ota-updater-macOS` artifact，解压后双击 `DS5Dongle OTA Updater.app`，不要直接双击裸二进制。
- Windows：下载 `ds5dongle-ota-updater-Windows` artifact，运行 `.exe`，release 构建不会弹命令行窗口。
- Linux：下载 `ds5dongle-ota-updater-Linux` artifact，可能需要给 HID 设备配置 udev 权限。

## 本地运行

```bash
cargo run --manifest-path tools/ota-updater/Cargo.toml
```

本地 `cargo run` 会从终端启动；这是开发运行方式，正式使用请下载对应平台 artifact。

macOS 本机打包：

```bash
cargo build --release --manifest-path tools/ota-updater/Cargo.toml
tools/ota-updater/package-macos.sh
```

脚本会生成 `tools/ota-updater/dist/DS5Dongle OTA Updater.app` 和对应 `.app.zip`，并做 ad-hoc codesign。

## OTA 协议

工具使用当前固件已经支持的报告：

- `0xF8`：读取固件版本。
- `0xF9`：读取 OTA 状态。
- `0xF6`：发送 OTA 命令。

OTA 包写入非活动 A/B 分区，设备端完成校验后才切换启动分区。
