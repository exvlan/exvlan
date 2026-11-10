# 发版检查清单（预览版）

> 以 7.3（`v26.410.20712-pre.8`）为模板。每一步都要在 **CI 绿** 或 **命令输出符合预期** 后才进入下一步。

## 0. 版本串自检

```bash
bash script/ci/build-version.sh                 # 分支：26.410.20712+<sha6>
GITHUB_REF_NAME=v26.410.20712-pre.8 bash script/ci/build-version.sh   # tag：26.410.20712-pre.8+<sha6>
```

- 标准版本（Cargo / package.json / tauri.conf / magisk）保持 `YY.QMM.Mmpp`
- 展示串 = `<标准版本>[-<channel>.<n>][+<sha6>]`；序号**基线内递增、换基线重置为 1**
- Windows MSI 用 `bundle.windows.wix.version`（四段纯数字，≤255/255/65535/65535），由
  `script/ci/msi-version.py` 在 CI 里按 tag 生成：
  - 预览版第 4 段 = `1000 + 预览序号`（pre.9 → `26.104.207.1009`）
  - 正式版第 4 段 = `60000 + patch`（`26.410.20712` → `26.104.207.60012`）
  - 这样任何新包都比旧包大，MSI 不会因为"同版本"走修复式安装

## 1. 本地检查

```bash
cargo check -p exvlan -p exvlan-studio -p exvlan-service
cargo test  -p exvlan-studio --lib
bash script/ci/check-paths.sh        # 绝对路径字面量只允许在 paths.rs
```

## 2. CI 流程

```bash
# 16 个提交全部在本地完成后一次推送，跑快门禁
git push origin develop

# 快门禁绿了再打 tag 跑全量
git tag v26.410.20712-pre.8
git push origin v26.410.20712-pre.8
```

## 3. Worker 部署与冒烟

本机只有 `cf` CLI 的 OAuth 登录态；`cf workers versions create` 要求
`cloudflare.config.ts`，读不了 `wrangler.jsonc`。实测 wrangler 可以直接把
这个 OAuth 令牌当作 `CLOUDFLARE_API_TOKEN` 用（写权限正常），所以统一走
`deploy.sh` → wrangler + `wrangler.jsonc`：

```bash
cd packages/diagnostics

# 一次性：设置签名密钥（值不要进仓库，也不要写进日志）
openssl rand -hex 32 | bash deploy.sh --set-secret TOKEN_SECRET

# 部署代码 + KV/ratelimits/routes（读 wrangler.jsonc）
bash deploy.sh
```

部署后立即冒烟（**全过才发预览包**）：

```bash
BASE=https://exvlan-diag.era-x.cn
curl -s $BASE/health                                  # {"ok":true}
TOKEN=$(curl -s $BASE/token | jq -r .token)           # 200 + token
curl -s -X POST "$BASE/zip?version=smoke&os=linux&arch=x86_64&summary=smoke" \
  -H "authorization: Bearer $TOKEN" -H 'content-type: application/zip' \
  --data-binary @<某个zip>                            # 200 + issue 链接
curl -s -o /dev/null -w '%{http_code}\n' -X POST "$BASE/zip" \
  -H 'x-diag-key: anything' --data-binary @<zip>      # 期望 401（旧 key 通道已关）
for i in $(seq 1 11); do curl -s -o /dev/null -w '%{http_code} ' $BASE/token; done  # 第 11 次 429
```

## 4. 产物

- 资产命名带预览标记：`…_26.104.207.12-pre.8_x64_en-US.msi`、`…_26.410.20712-pre.8_x64.dmg`
- Release：公开 prerelease，中文说明
- 核验：`.app` 内 `Resources/resources/service/exvlan-service` 与 `kernel/exvlan` 存在，minisign 可验

## 5. 验收门槛（4 条全过）

```
① macOS   点「上传诊断」→ GitHub issue 里出现链接
② macOS   退出重开 ×2 → 仍服务模式、0 弹窗
③ Android 打开网络 → 自动接管流量（不需要点横幅）
④ Windows 装 msi（1 次 UAC）→ 服务运行 → 点上传出链接
```

任一条不过 → 出下一个序号（`pre.9`），不改旧 tag。

## 6. 收口

```bash
# 清理所有旧预览版（保留刚验收通过的这一个）
gh release list -R exvlan/exvlan --limit 50
gh release delete <旧 tag> -R exvlan/exvlan --cleanup-tag
```

- 仓库可见性按需切回 private
- 服务端节点（relay / host）另行升级，不阻塞客户端发版
