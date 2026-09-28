# AGENTS.md — XMUHub 开发硬性规则

给所有在这个仓库里工作的 AI 编程助手（Codex、Claude、Cursor、Copilot……）和人看。
**下面的规则优先于任何任务描述。** 如果完成任务似乎必须违反其中一条，停下来，在回复 / PR 里说明原因，让项目负责人（vintcessun）决定，**不要自己想办法绕过**。
详细背景见 [docs/principles.md](docs/principles.md)。

## 1. 资料文件的字节绝不经过服务器（最重要）

服务器是一台出口带宽、磁盘、内存（服务进程上限 400 MB）都很小的机器。资料文件放在 GitHub Releases，**由用户的浏览器直接从国内 GitHub 镜像下载**。

禁止在服务器上做以下任何一件事，不管是作为主路径还是「兜底 / fallback / 降级方案」：

- 下载、转发、代理、流式中转资料文件（不管多大，不管有没有大小上限、并发上限）；
- 把资料文件整个或部分读进内存再发给浏览器；
- 在服务器磁盘上缓存、暂存、存储资料文件或它们的衍生物（缩略图、转码后的 PDF、解压出来的文件……）；
- 新增任何返回文件内容的接口（例如 `/api/.../preview/...`、`/api/.../file/...`、`/proxy?url=...`）。

允许的：服务器只返回**元数据和镜像链接**（JSON），浏览器自己去镜像取文件。

唯一的例外是已有的**上传流式中转**（`/api/relay/upload`，边收边转发到 GitHub、不落盘、有每日流量和并发上限），这是负责人明确同意过的。上传平时走 Cloudflare Worker（`upload.vintces.icu`），中转作为备用通道一直开着：Worker 失败，或者当天的 Worker 额度快用完时，才改走中转（同样经负责人同意）；用户头像（浏览器裁成 256×256、几十 KB）也走同一条上传流程存到 GitHub，同样经负责人同意。**不要扩大它的用途，也不要照着它再加新的例外。**

### 镜像不可用时该怎么办

预览或下载失败，根因几乎总是「镜像列表 / 镜像探测」出了问题。正确做法：

1. 查 `/api/admin/status` 里的镜像探测结果，看是哪几个镜像失败、错误是什么；
2. 修探测逻辑，或者在 `crates/xmuhub-server/src/config.rs` 的 `DEFAULT_MIRRORS` / `crates/xmuhub-core/src/storage/mirrors.rs` 的 `KNOWN_CORS` 里增删镜像（新镜像要先验证：返回的文件和 sha256 一致，预览用的还要带 `Access-Control-Allow-Origin`）；
3. 所有镜像都不行时，前端提示「暂时无法在线预览，请直接下载」——**这就是可以接受的结果**。

**错误做法：让服务器自己去下载文件再转给浏览器。** 这类改动一律不接受。

## 2. 服务器磁盘只放数据库

`/root/xmuhub/data` 里只有 redb 数据库。缩略图等衍生文件由 GitHub Actions（`XMUHub-transfer` 仓库的 workflow）生成并存到 GitHub Releases；服务器只记录它们在哪。

## 3. 内存

服务进程 `MemoryMax=400M`，平时只用几十 MB。不要按文件大小分配内存（`Vec::with_capacity(size)`、`bytes().await` 读整个响应体等），也不要引入常驻的大缓存。

## 4. 数据安全

- 改数据库结构或批量改数据之前，先备份线上数据库（`cp -a /root/xmuhub/data/xmuhub.redb /root/xmuhub/data.bak-<说明>-<时间>.redb`）。
- 已有的记录结构（`model.rs` 里用 postcard 序列化的 struct）**不要改字段**，否则旧数据读不出来；新功能加新表。
- 不要删除或覆盖 `data/`。
- **资料文件绝不能通过系统被删除**（负责人明确要求）：驳回、下架、删除申请、清理过期上传，一律只改状态 / 记录，GitHub Releases 上的文件一个字节都不删，所以任何决定都能「恢复发布」。存储后端故意没有 `delete` 接口，不要加回来；需要真正删除某个文件时由负责人在 GitHub 上手动处理。

## 5. 密钥和权限

- 所有密钥只放在 `.secrets/`（已 gitignore），永远不要提交，也不要写进代码、日志、提交信息。
- 管理员名单只在 `.secrets/admins.txt` 维护，用 `scripts/deploy.ps1 -AdminsOnly` 同步；不要把管理员邮箱写进仓库。
- 管理员之间平级，不能互相封禁 / 降级。

## 6. 前端

- 纯静态网页 + ES module，**没有构建步骤**，不要引入 npm 构建、打包器、框架。
- 第三方库放在 `web/vendor/<名字>-<版本>/`，固定版本、核对 npm 的 sha512，不要从运行时 CDN 加载。
  唯一的例外是注册页的 Cloudflare Turnstile（`challenges.cloudflare.com/turnstile/v0/api.js`）：它按设计只能从 Cloudflare 加载，只在用非常用邮箱注册、服务器要求人机验证时才加载，负责人同意过。

## 6.5 代码质量：Clippy 零警告

- 每次改 Rust 代码，提交前都要跑 `cargo clippy --workspace --all-targets`，**所有警告都要处理掉**（包括测试代码里的），不能留到以后。
- 优先改代码本身；`cargo clippy --fix` 自动改完要检查缩进和可读性。确实不该改的，用 `#[allow(clippy::xxx)]` 加在最小范围上，并在旁边写一句原因，不要整个文件或整个 crate 关掉。
- `cargo test --workspace` 同样要全部通过。

## 7. 部署

- 用 `pwsh scripts/sync.ps1`：它会同步 git，并按改动决定只更新网页还是重新编译。
- **Rust 代码改了就必须重新编译部署**，不能只用 `deploy.ps1 -WebOnly` 把依赖新接口的网页先发上去（会出现网页和后端版本不一致）。
- 推送前清掉 `GITHUB_TOKEN` 环境变量。
- 监听端口由 systemd 的 `xmuhub.socket` 持有（重启时新请求在端口上排队，不会 502），部署时是「先替换文件、再 `systemctl restart xmuhub.service`」。
  需要独占数据库时（比如在服务器上手动运行 `run` 的子命令），要**同时停掉** `xmuhub.socket` 和 `xmuhub.service`：只停服务的话，来一个请求 systemd 就会把它重新拉起来，和你抢数据库。

## 提交前自查

- [ ] 有没有任何代码路径让服务器下载 / 转发 / 缓存资料文件？（搜一下 `reqwest` + `bytes_stream` / `.bytes()` 出现在请求处理函数里的地方）
- [ ] 有没有新增返回文件内容的接口？
- [ ] 有没有改已有的数据库记录结构？
- [ ] 有没有把密钥或管理员邮箱写进仓库？
- [ ] `cargo clippy --workspace --all-targets` 是否零警告？`cargo test --workspace` 是否全部通过？
- [ ] `node scripts/check-web.mjs`（JS 语法、导入的名字是否真的被导出、页面引用的文件是否存在）和 `node scripts/check-rules.mjs`（本文件的硬性规则）是否通过？

以上检查在 GitHub Actions（`.github/workflows/ci.yml`）里对每次推送和 PR 自动运行，没通过的不要合并。`check-rules.mjs` 报「新文件用了网络请求」时，不要为了过检查去改写法，先问负责人；确认只是元数据请求后，由负责人把文件加进 `scripts/allowed-network-code.txt`。
- [ ] Rust 改了的话，是否按完整部署发布？

违反第 1 条的改动，即使「能用」「有上限」「只是兜底」，也会被直接回退。
