# 密钥和部署配置模板

把这个目录复制成 `.secrets/`（已被 git 忽略，永远不要提交），去掉每个文件名末尾的 `.example`，再按注释填写：

```
cp -r .secrets.example .secrets
cd .secrets && for f in *.example; do mv "$f" "${f%.example}"; done
```

| 文件 | 用途 | 必需 |
|---|---|---|
| `deploy.env` | 部署到哪台服务器、网站地址、用哪个 `site/` 文件夹、Cloudflare 区域 | 是 |
| `github.env` | 存资料文件的 GitHub 账号和令牌 | 是 |
| `upload.env` | 上传凭证的签名密钥、上传 Worker 名称、脚本令牌 | 是 |
| `cloudflare.env` | Cloudflare 账号、API 令牌、上传 Worker 域名、Turnstile 人机验证 | 用 Cloudflare 时 |
| `mail.env` | 发验证码邮件的 SMTP | 要邮箱注册时 |
| `admins.txt` | 管理员邮箱，一行一个 | 是 |
| `ddos-allow.txt` | 源站防护的放行地址（见 deploy/flood-guard/） | 可选 |

所有密钥都只在部署时写进服务器上的环境文件，不会编进程序，也不会进入仓库；换密钥只要改这里再部署一次。
