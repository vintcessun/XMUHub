# Cloudflare 上的配置（vintces.icu 区域）

xmu.vintces.icu 经 Cloudflare 代理（橙色云），其他子域名都是「仅 DNS」。下面这些都在 Cloudflare 后台 / API 里，仓库里没有对应的文件，改动时请同步更新这份说明。

## 缓存

| 设置 | 值 | 为什么 |
|---|---|---|
| Browser Cache TTL | Respect Existing Headers | 默认 4 小时会把我们的 `no-cache` 改成 `max-age=14400`，部署后浏览器拿旧 JS 配新页面，页面直接坏掉（2026-09-29 出过一次）。 |
| 缓存规则 1「XMUHub pages」 | 非 `/api/`、`/mcp`、`/d/`、`/cdn-cgi/`、`/assets/`、`/vendor/` 的 GET：边缘缓存 1 天，3xx–5xx 不缓存，浏览器缓存遵从源站 | 页面对所有人都一样（登录信息由页面脚本另取），只在部署时变化；刷首页的流量在边缘就挡掉。 |
| 缓存规则 2「XMUHub public API」 | `/api/tree`、`/api/meta`、`/api/recent`、`/api/popular`、`/api/links`、`/api/search`、`/api/nodes/*` 的 GET，**且没有 `xh_sid` Cookie、没有 `Authorization` 头**：边缘缓存 60 秒 | 只缓存访客看到的公开数据；登录用户、令牌、下载、上传、审核一律直达源站。 |

脚本、样式、字体、图片不需要规则：服务器给它们的地址加了内容哈希（`?v=…`，见 `crates/xmuhub-server/src/versioning.rs`），带当前哈希的请求返回 `immutable`，Cloudflare 按源站头缓存一年。

部署后 `scripts/deploy.ps1` 会清空整个区域的缓存（需要令牌有 Cache Purge 权限），带版本号的脚本和样式会被重新拉一次。

## 安全（WAF 自定义规则）

1. `/api` 和 `/mcp`（xmu.vintces.icu）：跳过 Browser Integrity Check，否则脚本和 MCP 客户端会被 1010 拦下。
2. `upload.vintces.icu`：跳过 Browser Integrity Check 和 Security Level。上传是页面后台发的跨域请求，弹不出人机验证；Worker 自己校验上传凭证。

## 其他

- 上传 Worker `xmuhub-upload` 绑定在自定义域名 `upload.vintces.icu`（`workers.dev` 在国内被污染）。部署见 `scripts/deploy.ps1 -OnlyWorker`；服务器从 `.secrets/cloudflare.env` 的 `CF_UPLOAD_DOMAIN` 得知它的地址。
- Turnstile 组件「XMUHub register」只允许 xmu.vintces.icu，密钥在 `.secrets/cloudflare.env`（`TURNSTILE_SITEKEY` / `TURNSTILE_SECRET`）。
- 令牌需要的权限：Workers、DNS、Zone WAF、Cache Rules、Cache Purge、Analytics（读）、Turnstile、区域设置读写。
