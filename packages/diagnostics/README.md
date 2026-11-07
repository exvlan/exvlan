# 诊断上传 Worker（`diag.exvlan.era-x.cn`）

接收 App「上传诊断」提交的日志，写入 **私有仓库 `exvlan/diagnostics`** 的 issue。

## 安全模型

- 目标仓库由 secret `REPO` 固定，**请求体不能指定仓库**
- 需要 `X-Diag-Key` 与 secret `UPLOAD_KEY` 一致（防随意灌 issue）
- GitHub token 建议用 fine-grained PAT：**只选 `exvlan/diagnostics`，只勾 Issues: Read and write**

## 部署

```bash
# secrets（只放在 Cloudflare，不进仓库）
cat > /tmp/diag-secrets.json <<'JSON'
{ "GH_TOKEN": "github_pat_xxx", "REPO": "exvlan/diagnostics", "UPLOAD_KEY": "<随机串>" }
JSON
cf deploy packages/diagnostics --secrets-file /tmp/diag-secrets.json
```

路由已在 `wrangler.jsonc` 中声明：`diag.exvlan.era-x.cn/*`（zone: era-x.cn）。

## 接口

```
GET  /health                    -> {"ok":true}
POST /   header: X-Diag-Key
  body: { version, os, arch, summary, errorLog, eventsTail, kernelTail }
  -> { urls: ["https://github.com/exvlan/diagnostics/issues/N", ...] }
```

正文超过 GitHub 64K 上限时自动拆分多条 issue。
