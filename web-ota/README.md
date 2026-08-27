# DS5Dongle Web OTA

Cloudflare Pages 静态版 WebHID OTA 工具。

运行时前端是 `index.html`。固件同步 Action 发布新版本后，会把 OTA 文件和 `manifest.json` 一起上传到 Cloudflare Pages：

- `manifest.json`：记录最新版本和保留的历史版本。
- `releases/<tag>/...`：保存 `.bin.ota` 和 SHA256 校验文件。

网页运行时只读取 Cloudflare 同源静态文件，不调用 GitHub API，也不通过 Worker 代理 GitHub Release 附件。

## 本地测试

```bash
npm test --prefix web-ota
```

## Cloudflare Pages 部署

```bash
RELEASE_TAG=v3.18 RELEASE_ASSETS_DIR=/path/to/release-assets node tools/build-web-ota-static.mjs
npx wrangler pages deploy web-ota-static --project-name ds5dongle-ota
```

需要环境变量：

```bash
CLOUDFLARE_API_TOKEN=...
CLOUDFLARE_ACCOUNT_ID=...
```

默认只保留最新 100 个版本，可用 `WEB_OTA_KEEP_RELEASES` 调整。
