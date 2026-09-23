# XMUHub

厦门大学学生资料共享平台。同学们可以按课程浏览、搜索、下载往年试卷、笔记和课件，也可以上传资料分享给大家。

线上地址：<https://xmu.vintces.icu>

> 非官方学生项目，与厦门大学官方无关。

## 设计要点

- **网站只管理资料，文件存在第三方。** 文件保存在 GitHub Releases（公开仓库），数据库只记录每个文件在哪里。存储层是可插拔的：一个文件的每个分卷可以有多个副本，放在不同的存储源上。
- **服务器不承担下载流量。** `/d/{id}` 和下载按钮都会跳转到国内 ghproxy 类镜像。服务器每 15 分钟通过各镜像下载一个 256KB 探针文件，按实测速度排序，全部不可用时回退到直连 GitHub。
- **上传不落盘。** 默认由服务器边收边转发到 `uploads.github.com`，不在本地存文件。上传有每日流量上限和并发上限。仓库里还有一个 Cloudflare Worker 版的中转（`worker/`）：`workers.dev` 在国内被污染无法访问，需要给它绑定自定义域名后才能切换过去，切换后服务器就完全不经手文件。
- **内容去重。** 文件以 sha256 作为主键，相同内容只存一份，重复上传会秒传。大文件在浏览器里切成不超过 95MB 的分卷，每个分卷单独校验，GitHub 返回的摘要必须与声明一致。
- **不需要注册，用令牌分级：**

  | 级别 | 获得方式 | 权限 |
  |---|---|---|
  | 访客 | — | 浏览、搜索、下载 |
  | 贡献者 | 网页自助领取 | 上传，**先审后发** |
  | 可信贡献者 | 审核员提升 | 上传，**先发后审** |
  | 审核员 | 管理员签发 | 审核队列、修改元数据、合并课程、下架、管理贡献者 |
  | 管理员 | CLI 签发 | 以上全部，外加签发任意令牌 |

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
crates/xmuhub-core    领域逻辑：数据模型、redb、搜索、令牌、存储后端（GitHub / 本地）、镜像测速
crates/xmuhub-server  Axum 服务：API、页面路由、上传中转、后台任务、CLI
web/                  前端静态文件
worker/               Cloudflare 上传中转 Worker（可选）
scripts/              编译（alinux3 容器）与部署脚本
```

## 本地开发

```bash
cargo run -p xmuhub-server -- token --level 4 --label dev   # 签发一个管理员令牌
cargo run -p xmuhub-server                                  # 默认使用本地存储，监听 127.0.0.1:8089
```

打开 <http://127.0.0.1:8089>，在“我的”页面粘贴令牌。默认的 `XMUHUB_STORAGE=local` 会把文件存到 `data/files/`，只用于开发。

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

## 编译与部署

密钥放在 `.secrets/` 下（已加入 gitignore）：`github.env`、`cloudflare.env`、`upload.env`。

```powershell
pwsh scripts/build-alinux3.ps1     # 在 Alibaba Cloud Linux 3 容器里编译 → ./run（只依赖 glibc）
pwsh scripts/deploy.ps1            # 上传 run 和 web/，安装 systemd 与 nginx 中转路由，重启并做健康检查
pwsh scripts/deploy.ps1 -WebOnly   # 只更新网页文件
pwsh scripts/deploy.ps1 -NewAdminToken   # 签发管理员令牌（会短暂停止服务）
```

部署时数据目录 `data/` 永远不会被覆盖。每次部署都会保留 `run.bak` 和 `web.bak`，便于回滚。元数据每天导出一份，推送到私有仓库 `XMUHub-backup` 做异地备份。

## 许可证

[GNU AGPL-3.0](LICENSE)。如果你修改后以网站形式对外提供服务，需要向使用者公开修改后的源代码。
