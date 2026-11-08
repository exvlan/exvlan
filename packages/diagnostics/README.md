# 诊断上传 Worker（`exvlan-diag.era-x.cn`）

接收 App「上传诊断」提交的日志，写入 **私有仓库 `exvlan/diagnostics`** 的 issue；
完整诊断 zip 存在 Cloudflare KV，issue 正文只放摘要 + 下载链接（GitHub API 不支持给 issue 附加文件）。

## 安全模型

- 目标仓库由 secret `REPO` 固定，**请求体不能指定仓库**
- 需要 `X-Diag-Key` 与 secret `UPLOAD_KEY` 一致（防随意灌 issue）
- GitHub token 建议用 fine-grained PAT：**只选 `exvlan/diagnostics`，只勾 Issues: Read and write**
- zip 下载链接用 32 位随机 id（`/diag/<id>`），知道链接即可下载，猜不到；KV 3 天自动过期

## 部署

```bash
# 1) KV namespace（只需创建一次，把 id 填进 wrangler.jsonc）
cf kv namespaces create --title exvlan-diag

# 2) secrets（只放在 Cloudflare，不进仓库）
cat > /tmp/diag-secrets.json <<'JSON'
{ "GH_TOKEN": "github_pat_xxx", "REPO": "exvlan/diagnostics", "UPLOAD_KEY": "<随机串>" }
JSON

# 3) 部署
cd packages/diagnostics && wrangler deploy
```

路由与 KV 绑定已在 `wrangler.jsonc` 中声明：
`exvlan-diag.era-x.cn/*`（zone: era-x.cn）+ `DIAG_KV`。

## 接口

```
GET  /health                    -> {"ok":true}

POST /         header: X-Diag-Key
  body: { version, os, arch, summary, errorLog, eventsTail, kernelTail }
  -> { urls: ["https://github.com/exvlan/diagnostics/issues/N", ...] }

POST /zip?version=&os=&arch=&summary=   header: X-Diag-Key, content-type: application/zip
  body: 完整诊断 zip（<= 25MB）
  -> { urls: ["https://github.com/exvlan/diagnostics/issues/N"],
       zipUrl: "https://exvlan-diag.era-x.cn/diag/<32位id>" }

GET  /diag/<id>                 -> application/zip（3 天后 404）
```

JSON 正文超过 GitHub 64K 上限时自动拆分多条 issue；zip 路径不做拆分（压缩包自身完整）。
