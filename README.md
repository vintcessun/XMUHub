# 鹭岛书阁

厦门大学学生资料共享平台。同学们可以按课程浏览、搜索、下载往年试卷、笔记和课件，也可以上传资料分享给大家。

线上地址：<https://xmu.vintces.icu>

> 非官方学生项目，与厦门大学官方无关。

## 设计要点

- **网站只管理资料，文件存在第三方。** 文件保存在 GitHub Releases（公开仓库），数据库只记录每个文件在哪里。存储层是可插拔的：一个文件的每个分卷可以有多个副本，放在不同的存储源上。
- **服务器不承担下载流量。** `/d/{id}` 和下载按钮都会跳转到国内 ghproxy 类镜像。服务器每 15 分钟通过各镜像下载一个 256KB 探针文件，按实测速度排序，全部不可用时回退到直连 GitHub。
- **上传不落盘。** 默认由服务器边收边转发到 `uploads.github.com`，不在本地存文件。上传有每日流量上限和并发上限。仓库里还有一个 Cloudflare Worker 版的中转（`worker/`）：`workers.dev` 在国内被污染无法访问，需要给它绑定自定义域名后才能切换过去，切换后服务器就完全不经手文件。
- **内容去重。** 文件以 sha256 作为主键，相同内容只存一份，重复上传会秒传。大文件在浏览器里切成不超过 95MB 的分卷，每个分卷单独校验，GitHub 返回的摘要必须与声明一致。
- **账号与分级权限。** 用邮箱验证码注册（任意邮箱都可以，厦大邮箱会标记“已认证”），密码用 argon2id 哈希保存，会话存在 HttpOnly Cookie 里，非 GET 请求需要带防 CSRF 的请求头。验证码邮件通过阿里云邮件推送（465 端口）从 `xmu@vintces.icu` 发出。

  | 级别 | 获得方式 | 权限 |
  |---|---|---|
  | 访客 | — | 浏览、搜索、下载 |
  | 贡献者 | 注册 | 上传，**先审后发** |
  | 可信贡献者 | 审核员提升 | 上传，**先发后审** |
  | 审核员 | 管理员提升 | 审核队列、编辑元数据、整理分类树、处理投诉、管理贡献者 |
  | 管理员 | **只由管理员名单决定** | 以上全部 |

  管理员名单是服务器上的一个文件（`XMUHUB_ADMINS_FILE`，每行一个邮箱），源文件放在不入库的 `.secrets/admins.txt`，用 `deploy.ps1 -AdminsOnly` 同步，服务每分钟重新读取一次。列入名单的账号自动成为管理员，移出名单的降为审核员；网页后台最多只能把人提升到审核员。

- **分类即目录，文件名由系统生成。** 分类树按团队的《资料分类方案 v1.6》组织：一级栏目 A 公共课 / B 专业课 / C 教材与参考书 / D 工具模板与校园服务，下面依次是课程组（或学院、学科）、课程、层次。A 类课程页按「01 真题与答案 / 02 提纲笔记 / 03 题库刷题 / 04 课件与拓展」分组展示。每份资料带一个 T1–T9 类型标签。上传者不需要自己起名，只要选择时间、类型、卷别、是否含答案，系统就会按 `课程_时间_类型(详情)` 生成规范文件名，同名的自动加 `_v2`。
- **合规状态**：`published`（公开）/ `pending`（待审）/ `restricted`（仅内部可见，搜不到）/ `removed`（已下架）/ `rejected`（已驳回）。原始文件名、来源和上传者只对审核员可见。每个资料页都有投诉入口。
- **批量上传**：一次拖入多个文件，按原文件名自动识别学年、学期、卷别、类型和是否含答案，也可以统一批量修改。

## 技术栈

| 部分 | 选型 |
|---|---|
| 后端 | Rust · Axum · Tokio |
| 数据库 | [redb](https://github.com/cberner/redb)（嵌入式，纯 Rust），记录用 postcard 编码并带版本号；读请求走内存，所有写入由一个写入者串行完成 |
| 搜索 | [Tantivy](https://github.com/quickwit-oss/tantivy)，索引放在内存里，启动时从 redb 重建。中文按单字和双字切分，拼音支持全拼和首字母（如 `gdsx` → 高等数学）；严格匹配没有结果时，退回到“查询里每个字都要出现”的模式，这样“高数”这类缩写也能搜到 |
| 内存 | mimalloc，调成立即把空闲内存还给系统，每分钟主动回收一次；systemd 限制 `MemoryMax=400M` 兜底。线上常驻内存约 25–40MB |
| 前端 | 不需要构建的静态 HTML/CSS/JS（`web/`）。服务启动时把它们读进内存、预先压缩成 brotli 和 gzip，带 ETag 返回。不依赖任何外部 CDN |

## 目录

```
crates/xmuhub-core    领域逻辑：数据模型、redb、搜索、账号、分类树、存储后端（GitHub / 本地）、镜像测速
crates/xmuhub-server  Axum 服务：API、页面路由、上传中转、后台任务、CLI
web/                  前端静态文件
worker/               Cloudflare 上传中转 Worker（可选）
scripts/              编译（alinux3 容器）、部署与资料导入脚本
```

## 本地开发

```bash
XMUHUB_ADMINS=you@example.com XMUHUB_SECURE_COOKIE=0 cargo run -p xmuhub-server   # 本地存储，监听 127.0.0.1:8089
```

打开 <http://127.0.0.1:8089> 注册（需要配置 `SMTP_*` 才能收到验证码），`XMUHUB_ADMINS` 里的邮箱注册后即为管理员。默认的 `XMUHUB_STORAGE=local` 会把文件存到 `data/files/`，只用于开发。

常用环境变量（完整列表见 `crates/xmuhub-server/src/config.rs`）：

| 变量 | 说明 |
|---|---|
| `XMUHUB_STORAGE` | `local` 或 `github` |
| `XMUHUB_BIND` / `XMUHUB_DATA` / `XMUHUB_WEB` | 监听地址、数据目录、网页目录 |
| `GH_STORE_USER` / `GH_STORE_TOKEN` | 存储账号和它的 token |
| `UPLOAD_TICKET_SECRET` | 签名上传凭证用的密钥 |
| `UPLOAD_WORKER_URL` | 留空时由服务器中转上传；填 Worker 地址则改走 Worker |
| `RELAY_DAILY_MB` / `RELAY_CONCURRENCY` | 服务器中转的每日流量上限、并发上限 |
| `MIRRORS` | 下载镜像前缀列表，用逗号分隔 |
| `XMUHUB_ADMINS_FILE` | 管理员名单文件（另可用 `XMUHUB_ADMINS` 以逗号分隔追加） |
| `SMTP_HOST` / `SMTP_PORT` / `SMTP_USER` / `SMTP_PASSWORD` / `MAIL_FROM` | 验证码邮件的外发 SMTP |
| `XMUHUB_SCRIPT_TOKEN` | 自动化脚本（导入工具）使用的管理员 Bearer 令牌 |

## 编译与部署

密钥放在 `.secrets/` 下（已加入 gitignore）：`github.env`、`cloudflare.env`、`upload.env`、`mail.env`，以及管理员名单 `admins.txt`。

```powershell
pwsh scripts/build-alinux3.ps1     # 在 Alibaba Cloud Linux 3 容器里编译 → ./run（只依赖 glibc）
pwsh scripts/deploy.ps1            # 上传 run 和 web/，安装 systemd 与 nginx 中转路由，重启并做健康检查
pwsh scripts/deploy.ps1 -WebOnly   # 只更新网页文件
pwsh scripts/deploy.ps1 -AdminsOnly   # 只同步管理员名单
pwsh scripts/deploy.ps1 -SetRole a@b.com -Level 3   # 命令行设置 1–3 级角色（也可以在 /admin 页面操作）
```

部署时数据目录 `data/` 永远不会被覆盖。每次部署都会保留 `run.bak` 和 `web.bak`，便于回滚。元数据每天导出一份，推送到私有仓库 `XMUHub-backup` 做异地备份。

## 导入已整理的资料库

```bash
python scripts/import_archive.py 资料库备份.zip --base https://xmu.vintces.icu [--dry-run] [--limit N]
```

导入脚本按目录建立分类树，把文件名解析成结构化字段，然后从本机直接上传到 GitHub（不经过服务器）。规则如下：

- 原始文件名（来自 `重命名对照表.csv`）含「勿外传」「仅内部」「密码」的，同名的所有版本都标为 `restricted`；
- 文件名带「（不确定）」的标为待审并打上“待核实”；
- 其余直接公开。

导入进度保存在压缩包旁边，中断后重新运行会从断点继续。

## MCP 与个人令牌

登录后在「我的」页面创建个人令牌（`xmh_…`，只显示一次，可随时删除）。令牌以本人身份和权限访问：

- **MCP**：`https://xmu.vintces.icu/mcp`（Streamable HTTP，JSON 响应），请求头 `Authorization: Bearer xmh_…`；不带令牌也能搜索、浏览和取下载地址。
  工具：`search`、`get_tree`、`get_category`、`get_resource`、`get_download_links`、`list_recent`、`list_popular`、`get_comments`、`post_comment`、`rate_resource`、`submit_feedback`、`whoami`，审核员另有 `review_queue`、`review_resource`。
- **REST API**：同一令牌可直接调用 `/api/…`（带 Bearer 的请求免 `X-XMUHub` 头）。令牌不能再创建令牌。

```sh
claude mcp add --transport http ludao-shuge https://xmu.vintces.icu/mcp --header "Authorization: Bearer xmh_…"
```

## 许可证

[GNU AGPL-3.0](LICENSE)。如果你修改后以网站形式对外提供服务，需要向使用者公开修改后的源代码。
