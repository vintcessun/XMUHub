# 给自己的学校部署一份：fork 教程

这个仓库（[vintcessun/XMUHub](https://github.com/vintcessun/XMUHub)）是**上游**。你 fork 之后只需要改两样东西：

- `site/<你的站>/`：站名、学校、域名、标语、Logo、背景图……所有「你这个站自己的东西」；
- `.secrets/`：服务器地址和各种密钥（不入库）。

其余的 `web/`、`crates/`、`scripts/` 都是所有站共用的代码，**不要改**。这样上游每次更新，你执行一次 `git merge upstream/main` 就能同步，不会有冲突。想改共用代码的话，欢迎提 PR 回上游，大家一起用。

## 0. 准备

| 需要 | 说明 |
|---|---|
| 一台 Linux 服务器 | 2 核 2 GB 足够（程序常驻内存约 30–40 MB），需要 systemd 和 nginx（宝塔面板也可以） |
| 一个域名 | 建议接入 Cloudflare（免费版即可），用于缓存和防护 |
| 一个专门存文件的 GitHub 账号 | 资料文件存在它的 GitHub Releases 里，由浏览器从国内镜像直接下载，不占服务器流量 |
| 你自己的电脑 | 装 [PowerShell 7](https://github.com/PowerShell/PowerShell)（`pwsh`）、Git、[Docker](https://www.docker.com/)（用来编译服务器程序）；Windows、macOS、Linux 都行 |
| 可选 | Cloudflare Worker（上传中转）、Turnstile（人机验证）、SMTP（发注册验证码） |

## 1. Fork 并克隆

在 GitHub 上点 **Fork**，然后：

```sh
git clone https://github.com/<你>/XMUHub.git
cd XMUHub
git remote add upstream https://github.com/vintcessun/XMUHub.git
```

## 2. 建立你的站点文件夹

```sh
cp -r site/example site/<你的站>      # 文件夹名用英文，比如 site/fzu
```

- 编辑 `site/<你的站>/site.json`：每一项的含义见 [site/README.md](../site/README.md)。`repo` 改成你 fork 的地址，`verified_domains` 填你们学校的邮箱域名。
- 放你的图片（可选，不放就用默认的简洁样式）：`site/<你的站>/assets/site/logo.*`、`background.*`（png / jpg / webp 任选，同名只放一个）、`home-left.png`、`home-right.png`。
- 想整页改「使用须知」「使用教程」等页面，就把 `web/about.html` 之类复制到 `site/<你的站>/about.html` 再改。它会覆盖默认页面，但以后上游对这个页面的改动你需要自己合并，所以能用 `site.json` 解决的尽量用 `site.json`。

本地预览（不需要任何密钥，文件存在本地 `data/`）：

```sh
XMUHUB_SITE=<你的站> XMUHUB_SECURE_COOKIE=0 XMUHUB_ADMINS=you@example.com cargo run -p xmuhub-server
# 打开 http://127.0.0.1:8089
```

检查站点配置有没有写错：

```sh
node scripts/check-web.mjs
```

把站点文件夹提交到你的 fork（它是公开的网站内容，可以入库）：

```sh
git add site/<你的站> && git commit -m "我的站点" && git push
```

## 3. 填写密钥和部署配置

```sh
cp -r .secrets.example .secrets
cd .secrets && for f in *.example; do mv "$f" "${f%.example}"; done && cd ..
```

然后按每个文件里的注释填写，说明见 [.secrets.example/README.md](../.secrets.example/README.md)。最重要的是 `deploy.env`：

```ini
SSH_HOST=你的服务器          # 需要能用 ssh 密钥免密登录
PUBLIC_URL=https://你的域名
SITE=<你的站>               # 第 2 步的文件夹名
```

`.secrets/` 已经被 git 忽略，**永远不要提交**。密钥只在部署时写进服务器上的环境文件，不会编进程序。

## 4. 准备服务器

1. **nginx**：给你的域名建一个站点，把请求反向代理到 `http://127.0.0.1:8089`（端口可以在 `deploy.ps1 -AppPort` 改）。经 Cloudflare 代理时，参考 [deploy/nginx/](../deploy/nginx/) 让 nginx 认出访问者的真实 IP 并限流。用宝塔的话，在 `deploy.env` 里填 `NGINX_EXT_DIR`，部署脚本会自动放好上传中转的路由。
2. **Cloudflare**（推荐）：域名开启代理（橙色云），然后在 `deploy.env` 填好 `CF_ZONE`、`CF_HOST`，运行 `python3 scripts/cf_cache_rules.py apply`，自动设置缓存规则。详见 [deploy/cloudflare.md](../deploy/cloudflare.md)。
3. **源站防护**（可选）：见 [deploy/flood-guard/](../deploy/flood-guard/)。

## 5. 部署

```sh
pwsh scripts/sync.ps1
```

第一次会在 Docker 里编译服务器程序（几分钟），然后上传程序、网页和你的站点文件夹，安装 systemd 服务并启动。以后每次运行它都会自动判断：只改了网页或站点文件夹就只更新网页，改了程序才重新编译。

管理员：把邮箱写进 `.secrets/admins.txt`，运行 `pwsh scripts/deploy.ps1 -AdminsOnly`，用这个邮箱注册的账号就是管理员。

## 6. 同步上游更新

```sh
git fetch upstream
git merge upstream/main
pwsh scripts/sync.ps1
```

只要你没改 `web/`、`crates/`、`scripts/` 里的共用文件，合并就不会有冲突。上游如果给 `site.json` 加了新的必填项，`node scripts/check-web.mjs`（CI 也会跑）和服务器启动时都会明确告诉你缺哪一项，照着 `site/example/site.json` 补上即可。

## 能改什么、不要改什么

| 位置 | 能不能改 |
|---|---|
| `site/<你的站>/` | ✅ 随便改，这就是你的站 |
| `.secrets/` | ✅ 你自己的，不入库 |
| `web/`、`crates/`、`scripts/`、`worker/` | ❌ 共用代码，改了会和上游冲突；需要的功能请提 Issue 或 PR |
| `site/xmu/`、`site/example/` | ❌ 上游自己的站和模板，别动 |

另外，`web/` 和程序代码里如果写死了任何一个站的站名、学校、域名，CI 会报错，所以提 PR 时也请用 `{{site.…}}` 占位。

## 许可证

[GNU AGPL-3.0](../LICENSE)：你修改后以网站形式对外提供服务，需要向使用者公开修改后的源代码。页面上的「开源」链接指向 `site.json` 里的 `repo`，填你自己的 fork 就满足这一条。
