# 源站防护（2026-09-29 装在服务器上）

有人拿到服务器 IP 后绕过 Cloudflare，直接对 80/443 建大量空连接，占满连接跟踪表，导致 Cloudflare 回源 522/525、ssh 连不上。

服务器上：

- `/etc/sysctl.d/90-xmuhub-ddos.conf`：连接跟踪表 262144，空闲连接 10 分钟过期（原来 5 天）。
- `xmuhub-ddos-rules.service`（开机运行 `/usr/local/sbin/xmuhub-ddos-rules`）：Cloudflare 的 IP 段和 `/etc/xmuhub/ddos-allow` 里的地址放行（这个文件的内容在 `.secrets/ddos-allow.txt`，不进仓库；改完上传：`ssh root@vintces.icu "cat > /etc/xmuhub/ddos-allow" < .secrets/ddos-allow.txt`）；其他来源对 80/443 同时最多 100 个连接；`ddos_ban` 名单里的来源在 raw 表直接丢弃。
- `xmuhub-ddos-ban.timer`（每分钟运行 `/usr/local/sbin/xmuhub-ddos-ban`）：先补回被防火墙重载冲掉的规则，再把同时占用超过 100 个连接的来源封 1 小时（自动过期）。封禁记录：`journalctl -t xmuhub-ddos`。

重装：把 `xmuhub-ddos-rules`、`xmuhub-ddos-ban` 传到服务器 `/tmp/`，再 `bash install.sh`。
Cloudflare 的 IP 段见 https://www.cloudflare.com/ips/ ，有变化时同步。

根治办法是阿里云安全组只放行 Cloudflare 的 IP 段访问 80/443。
