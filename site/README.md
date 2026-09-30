# 站点目录 `site/`

每个部署的站点在这里有自己的文件夹（上游自带的是 `xmu/`，即 xmu.vintces.icu）。**站名、学校、域名、标语、图片等所有「这个站自己的东西」都只放在这里**，`web/` 和 `crates/` 里的代码是所有站共用的。这样 fork 的人只改自己的文件夹，同步上游时不会冲突。怎么 fork、部署、同步见 [docs/fork.md](../docs/fork.md)。

## 一个站点文件夹里有什么

```
site/<你的站>/
  site.json          必需：站点信息（见下表）
  assets/site/       可选：图片，同名替换默认图
    logo.*             256×256，页头和浏览器图标（png / jpg / webp / svg 都行，只放一个）
    background.*       全站背景，建议 1920×1280、200–400 KB（png / jpg / webp 都行，只放一个）
    home-left.png      首页标题两侧的装饰图（透明 PNG，约 660 px 宽）；不要就别放
    home-right.png
  about.html …       可选：整页替换 web/ 里的同名页面（一般不需要）
```

服务器启动时先读 `web/`，再用站点文件夹里的同路径文件覆盖（`site.json` 和 `*.md` 除外），然后把页面、脚本、样式里的 `{{site.<键>}}` 换成 `site.json` 里的值。每个站只在启动时处理一次，之后直接从内存发送，不影响访问速度。

选哪个站点：服务器上由部署脚本放在 `web/_site/`；本地运行时用环境变量 `XMUHUB_SITE=<文件夹名>`（默认 `xmu`），或 `XMUHUB_SITE_DIR=<路径>`。

## site.json

| 键 | 用在哪 | 例子（`example/`） |
|---|---|---|
| `name` | 站名：标题、页头、邮件、MCP；普通用户不能用它当昵称 | 某某书阁 |
| `subtitle` | 页头站名下面的小字 | 学生资料库 |
| `school` / `school_short` | 学校全称 / 简称 | 某某大学 / 某大 |
| `description` | 每个页面的 `<meta name="description">` | … |
| `slogan` | 首页站名下面的一句话 | … |
| `footer` | 页脚第一行 | … |
| `domain` | 网站域名（不带 https://），用于页面上的示例和 MCP | hub.example.com |
| `repo` | 页面上「开源」链接指向的仓库，fork 的人填自己的 | https://github.com/your-name/XMUHub |
| `mcp_name` | MCP 服务名（`claude mcp add … <mcp_name> …`） | example-hub |
| `community` | 意见反馈旁边的交流群等联系方式，可以留空 | QQ 群：123456 |
| `icp` | ICP 备案号，显示在每页底部并链接到工信部备案网站；境内服务器或域名必须填写，可以留空 | 闽ICP备2025000000号 |
| `verified_domains` | 学校邮箱域名（含子域名），用它注册的账号显示认证标记，注册也不需要人机验证 | ["example.edu.cn"] |
| `verified_label` | 认证标记的文字 | 某大认证 |
| `link_example` | 「站外资源」推荐表单的名称示例 | 如 某某学院历年试卷合集 |
| `announcement` | 管理员设置公告之前默认显示的公告 | … |
| `extra` | 可选：自定义的其他文字，页面里用 `{{site.extra.<键>}}` | {"qq": "123456"} |

值里不能有 `< > " ' \` \ { }` 和换行（会直接放进 HTML 和脚本里）；服务器启动和 `node scripts/check-web.mjs` 都会检查。`web/` 和程序代码里写死任何一个站的站名、学校、域名、联系方式，CI 也会报错——请用 `{{site.…}}`。
