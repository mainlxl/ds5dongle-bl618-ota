# DS5Dongle Web OTA

Cloudflare Pages 版 WebHID OTA 工具。

运行时前端是 `index.html`，Release 查询和固件下载走 Cloudflare Pages `_worker.js`：

- `/api/latest`：通过 GitHub `/releases/latest` 跳转解析最新 tag，不使用 GitHub API。
- `/api/download`：由 Cloudflare 服务端代理下载 GitHub Release 附件，浏览器只访问同源地址。

这样可以避开浏览器直接 `fetch` GitHub Release 附件时遇到的 CORS 限制。

## 本地测试

```bash
npm test --prefix web-ota
```

## Cloudflare Pages 部署

```bash
npx wrangler pages deploy web-ota --project-name ds5dongle-ota
```

需要环境变量：

```bash
CLOUDFLARE_API_TOKEN=...
CLOUDFLARE_ACCOUNT_ID=...
```
