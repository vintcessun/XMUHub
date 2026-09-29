# xmu.vintces.icu 的 nginx 配置（Cloudflare 代理）

xmu.vintces.icu 经 Cloudflare 代理（DNS 橙色云），其他子域名都是「仅 DNS」。
这里的文件是**手动安装**的，`deploy.ps1` 只管 `xmuhub-relay.conf`，不会覆盖它们。

| 文件 | 装到 | 作用 |
|---|---|---|
| `0.xmuhub-limits.conf` | `/www/server/panel/vhost/nginx/`（http 级） | 限流区：页面和接口按访问者 IP 每秒 10 次，静态文件不计；耗时日志的格式 |
| `logrotate-xmuhub-timing` | `/etc/logrotate.d/xmuhub-timing` | 耗时日志每天轮转，留 7 天 |
| `xmuhub-cloudflare.conf` | `/www/server/panel/vhost/nginx/extension/xmu.vintces.icu/`（站点级） | 只信任 Cloudflare 网段发来的 `CF-Connecting-IP` 作为真实 IP；启用限流（瞬时可多 80 次，超出返回 429） |

安装 / 更新（先备份，`nginx -t` 通过才重载）：

```sh
tar czf /root/nginx-vhost-bak-$(date +%Y%m%d%H%M%S).tgz -C / www/server/panel/vhost/nginx
cp 0.xmuhub-limits.conf /www/server/panel/vhost/nginx/
cp xmuhub-cloudflare.conf /www/server/panel/vhost/nginx/extension/xmu.vintces.icu/
nginx -t && nginx -s reload
```

Cloudflare 的网段偶尔会变，列表来自 <https://api.cloudflare.com/client/v4/ips>，变了就重新生成 `set_real_ip_from` 那几行。
没有这份配置时，服务器看到的访问者 IP 全是 Cloudflare 的，按 IP 的限制（验证码、注册、限流）会把所有人算成同一个人。

## 暂不启用：只接受来自 Cloudflare 的请求

源站 IP 已经暴露，攻击者可以绕过 Cloudflare 直接打服务器。需要时在 `xmuhub-cloudflare.conf` 末尾加上下面这段，
非 Cloudflare 来源直接断开连接、不记日志（`real_ip` 生效前的原始地址是 `$realip_remote_addr`）：

```nginx
# 在 0.xmuhub-limits.conf（http 级）里：
geo $realip_remote_addr $xmuhub_from_cf {
    default 0;
    # 与 set_real_ip_from 相同的 Cloudflare 网段，每行 "<网段> 1;"
}
# 在 xmuhub-cloudflare.conf（站点级）里：
if ($xmuhub_from_cf = 0) { return 444; }
```

启用前确认所有解析都已经切到 Cloudflare（各地 DNS 缓存可能要 1–2 天），否则还在直连的同学会打不开网站。

## 请求耗时日志

`/www/wwwlogs/xmu.vintces.icu.timing.log`，每行：时间、访问者、请求、状态、字节数、`rt`（整个请求）、`ut`（XMUHub 处理）、`uc`（连上 XMUHub）。

```sh
# 最近最慢的 20 个请求
tail -n 20000 /www/wwwlogs/xmu.vintces.icu.timing.log | awk '{for(i=1;i<=NF;i++) if($i~/^rt=/){split($i,a,"="); print a[2], $0}}' | sort -rn | head -20
```

`rt` 远大于 `ut`：慢在网络或访问者那边；`ut` 大：慢在程序（比如在排队写数据库）。
