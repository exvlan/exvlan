# 镜像工具链（Bun 单一真源）+ `/mirror/trigger`

> 范围：`script/kernel-mirror/**`（同步实现、e2e、站点文件、unit）+ `script/ci/mirror-trigger.sh`
> + `.github/workflows/pipeline.yml` 的一个 job。
> 通道语义仍以 [`docs/decisions-7.3.md`](./decisions-7.3.md) §14.13（通道化）/§14.12（清理）为准，
> 本文只讲**实现与部署**，不改语义。

## 0. 结论

- 同步实现从「shell + 4 个 Python 脚本」收敛成 **`script/kernel-mirror/index.ts`（Bun）**，
  CLI 参数与环境变量保持完全兼容（发布机换镜像不需要改任何 env）。
- 新实现与旧实现**在同一份假 Release 上产出的 dist 树逐字节一致**（只归一化时间戳与绝对路径字段），
  由 `test/parity.test.ts` 自动验证（5 个场景）。
- 旧 e2e 的 **62 条断言逐条**搬到 `bun test`（`test/e2e-sync.test.ts`，62 个 `test()`）。
- 旧 `.sh` / `.py` **保留一个周期**并标注 legacy（文件头 `⚠️ legacy`），下一个 minor 删除。
- 新增 `/mirror/trigger`：`POST` 触发同步、`GET` 读最近一次状态；CI 在发版后触发并**最多轮询 120s**，
  失败只告警（定时同步兜底），不会卡死 release。

跑一遍（离线，不需要 token）：

```bash
bun --version                      # 1.4.4
bun test script/kernel-mirror      # 123 pass（62 e2e + 5 parity + 30 units + 13 entry + 13 trigger）
bash script/kernel-mirror/e2e-sync-test.sh   # legacy 对照：62 项全绿
```

## 1. 新结构（文件清单 + 职责）

| 文件 | 职责 |
| --- | --- |
| `index.ts` | **入口**：`--dry-run` / `--tags-file` / `-h`，env 与旧 shell 一字不差；出错退出码 1 |
| `trigger-server.ts` | `/mirror/trigger`：鉴权、防重入、异步跑同步、可选周期同步、脱敏状态回传 |
| `src/config.ts` | CLI/env → `Config`（`USAGE`、`KEEP_CORE_STABLE`/`KEEP_APP_STABLE`、`KEEP` 回落、`PLATFORMS` 默认 6 平台） |
| `src/log.ts` | `[sync]` 前缀日志（沿用旧文案，看门狗/排错 grep 不变）+ `redact` / `redactSecrets` |
| `src/pyjson.ts` | `json.dumps(obj, indent=2, ensure_ascii=False)` 的等价序列化（`": "` 分隔、空容器 `[]/{}`、`\u00xx` 小写、非 ASCII 原样） |
| `src/version.ts` | `sort -V` 等价版本比较（`SINCE_TAG`、通道内倒序） |
| `src/tags.ts` | tag→通道归类、`KEEP_<CHANNEL>` 窗口、`SINCE_TAG`（**只约束 stable**）、`MAX_BYTES` 预算收缩 |
| `src/gh.ts` | `gh api repos/$REPO/releases?per_page=100 --paginate --jq …` 与 `gh release download … -p …`（仍走 CLI） |
| `src/manifest.ts` | `merge_manifests` + `filter_manifest_platforms` + 复刻 `grep/sed` 的清单字段抽取 |
| `src/channelize.ts` | URL 通道化（`<root>/<channel>/<文件名>`，保留 origin、丢 query/fragment、幂等） |
| `src/fsx.ts` | 原子写 / `ln -sfn` + rename / `du -sb` 等价的表观大小 / C-locale `sort` / sha256 |
| `src/publish.ts` | `publish_core` / `publish_app`：先资产后清单、半发布防御、未版本化资产归属、按引用清理 |
| `src/status.ts` | `mirror-status.json` 新旧 schema 双写（新 schema 保留看门狗写的其它键与键序） |
| `src/run.ts` | 主流程：取元数据 → 选窗口 → 下载 → 发布 → 增量状态 → 别名/旧布局链接 → 状态；可选跨进程锁 |
| `test/harness.ts` | 假 `gh` + 假 Release 夹具 + 「跑旧实现/跑新实现」运行器 |
| `test/e2e-sync.test.ts` | 62 条断言（与旧 e2e 逐条对应，见 §3） |
| `test/parity.test.ts` | 新旧实现 dist 树逐字节对比（5 个场景） |
| `test/units.test.ts` | 单元：合并/裁剪/通道化/版本/Python JSON/状态双写/通道选择 |
| `test/entry.test.ts` | 入口兼容与边界：参数、env 覆盖、空通道、nightly、`VERIFY_SHA256` |
| `test/trigger.test.ts` | trigger：401/202/409/405、状态、锁、超时、脱敏 |
| `test/fixtures/fake-sync.ts` | trigger 用的假同步（可配耗时/结果，故意打印 token 供脱敏断言） |
| `caddy.Caddyfile` | 站点文件（`/core/*`、`/upgrade/*`、`/mirror/trigger` 反代） |
| `exvlan-kernel-mirror.service` / `.timer` | 非容器部署的定时同步（保留） |
| `exvlan-kernel-mirror-trigger.service` | **新增**：非容器部署的 trigger 单元（只接受按需触发） |
| `script/ci/mirror-trigger.sh` | CI 侧触发 + 120s 就绪轮询（失败只告警） |
| legacy（保留一周期） | `exvlan-kernel-sync.sh`、`merge_manifests.py`、`filter_manifest_platforms.py`、`channelize_urls.py`、`mirror_status.py`、`e2e-sync-test.sh` |

### 行为不变式（写代码前先读，改坏任何一条都会静默出错）

1. **通道目录** `<DEST>/<channel>/manifest.json` + `*.zip`；`<UPGRADE_DEST>/<channel>/{latest,manifest,magisk_update}.json` + 资产。
2. **`SINCE_TAG` 只约束 stable**；其余通道按 `KEEP_<CHANNEL>`（nightly 固定 1）。
3. **空通道不建目录**（无 tag → 发布阶段直接返回 `noop`）。
4. **别名是符号链接**（`<DEST>/manifest.json → stable/manifest.json` 等），原子替换，不复制文件。
5. **未版本化资产只由通道最新 tag 发布**（`app-*-release*.apk` / `*.msi` / `*-setup.exe` / `*.app.tar.gz`）。
6. **`*.sig` 跟随资产一起收、一起清**：清单引用 + `latest.json` 的 `url` + `magisk_update.json` 的 `zipUrl`，
   每个名字再配对 `<name>.sig`（缺失即触发半发布防御，保留旧内容）。
7. **半发布防御**：清单/`latest.json`/`magisk_update.json` 引用的文件不在 stage 也不在已发布目录 → 整轮放弃（保留旧内容）。
8. **`MAX_BYTES`（默认 8 GiB）是窗口预算**：超限从最旧版本收缩 core/app 窗口，清单同步收缩。
9. **状态双写**：`<DEST>/mirror-status.json`（新 schema，`channels.*`）+ `<EXVLAN_DIST>/mirror-status.json`（旧 schema，
   看门狗按 mtime 判活）。**所有早退路径都要写**。
10. **`core/<zip>` 缺失时补 `stable/<name>` 的相对符号链接**（老客户端与看门狗按根路径找）。
11. **增量指纹**：`sort assets.<tag> | sha256`；nightly 另看 `.published-at`。只补下旧 tag 时 latest/magisk **不降级、不粘住**。
12. **`--dry-run` 不下载、不写盘**；`--tags-file` 跳过 `gh api`（无资产元数据 → 放弃预算裁剪、总是下载）。
13. **`sha256` 校验**：默认只做「引用文件存在」检查（与旧脚本一致）；`VERIFY_SHA256=1` 额外核对清单声明的 sha256，
    **不一致只告警**，不改变发布结果。签名的**密码学校验**不在镜像端做（没有公钥，客户端/服务端各自校验）。

## 2. 等价性证明（新旧 dist 树 diff 必须为空）

`test/parity.test.ts` 对同一份假 Release 分别跑：

- 旧：`bash script/kernel-mirror/exvlan-kernel-sync.sh`（shell + 4 个 Python）
- 新：`bun script/kernel-mirror/index.ts`

然后逐项比较：**文件集合、每个文件内容 sha256、符号链接目标、JSON 排版（Python 兼容序列化）、
`.sync-state` 指纹、`bytes.*` 统计**。只归一化两类天然不同的值：

| 归一化字段 | 原因 |
| --- | --- |
| `updated` / `checked_at` / `first_ok_at` / `last_change_at` / `wait_since` | 两边跑的时刻不同 |
| `dest` / `upgrade_dest` | 两棵对比树在不同目录（`parity-a-legacy` vs `parity-a-new`） |

场景（全部为空 diff）：首轮两版 / `FORCE=1` 全量重下 / 增量轮（只重下旧 tag）/ `MAX_BYTES` 预算裁剪 / `SINCE_TAG`。

```bash
$ bun test script/kernel-mirror/test/parity.test.ts
 5 pass
 0 fail
```

补充：`bytes.*` 用的是 `du -sb` 的**表观大小**语义（普通文件 st_size + 符号链接目标长度，**不含目录自身**），
已实测对齐；`.sync-state` 指纹用 C-locale 的 `sort`（逐字节）后再 `sha256`，与 `sort | sha256sum` 一致。

## 3. 「原 62 条断言 → 新测试」对照表

旧 e2e 的 `pass()` 输出共 62 条，新 `test/e2e-sync.test.ts` 是 62 个 `test()`（顺序一致、名称尽量沿用原文）。
`bun test` 报告：

```
$ bun test script/kernel-mirror/test/e2e-sync.test.ts
 62 pass
 0 fail
Ran 62 tests across 1 file.
```

| # | 场景 | 旧断言（e2e-sync-test.sh） | 新测试（bun test › describe › test） |
| --- | --- | --- | --- |
| 1 | A. 首轮发布 v26.410.20711 + v26.410.20710 | 两版都下载了 | `A. 首轮发布 v26.410.20711 + v26.410.20710` › 两版都下载了 |
| 2 | A. 首轮发布 v26.410.20711 + v26.410.20710 | 最新版也下载了 | `A. 首轮发布 v26.410.20711 + v26.410.20710` › 最新版也下载了 |
| 3 | A. 首轮发布 v26.410.20711 + v26.410.20710 | latest.json 版本 | `A. 首轮发布 v26.410.20711 + v26.410.20710` › latest.json 版本 |
| 4 | A. 首轮发布 v26.410.20711 + v26.410.20710 | magisk_update.json 版本 | `A. 首轮发布 v26.410.20711 + v26.410.20710` › magisk_update.json 版本 |
| 5 | A. 首轮发布 v26.410.20711 + v26.410.20710 | 未版本化资产 app-arm64-release.apk 仍是 26.410.20711 | `A. 首轮发布 v26.410.20711 + v26.410.20710` › 未版本化资产 app-arm64-release.apk 仍是 26.410.20711 |
| 6 | A. 首轮发布 v26.410.20711 + v26.410.20710 | 未版本化资产 app-arm-release.apk 仍是 26.410.20711 | `A. 首轮发布 v26.410.20711 + v26.410.20710` › 未版本化资产 app-arm-release.apk 仍是 26.410.20711 |
| 7 | A. 首轮发布 v26.410.20711 + v26.410.20710 | 未版本化资产 app-x86_64-release.apk 仍是 26.410.20711 | `A. 首轮发布 v26.410.20711 + v26.410.20710` › 未版本化资产 app-x86_64-release.apk 仍是 26.410.20711 |
| 8 | A. 首轮发布 v26.410.20711 + v26.410.20710 | 未版本化资产 app-x86-release.apk 仍是 26.410.20711 | `A. 首轮发布 v26.410.20711 + v26.410.20710` › 未版本化资产 app-x86-release.apk 仍是 26.410.20711 |
| 9 | A. 首轮发布 v26.410.20711 + v26.410.20710 | 未版本化资产 msi 仍是 26.410.20711 | `A. 首轮发布 v26.410.20711 + v26.410.20710` › 未版本化资产 msi 仍是 26.410.20711 |
| 10 | A. 首轮发布 v26.410.20711 + v26.410.20710 | 未版本化资产 setup.exe 仍是 26.410.20711 | `A. 首轮发布 v26.410.20711 + v26.410.20710` › 未版本化资产 setup.exe 仍是 26.410.20711 |
| 11 | A. 首轮发布 v26.410.20711 + v26.410.20710 | upgrade/manifest.json 最新版 sha256：26.410.20711: 6 个平台 sha256 全部 MATCH | `A. 首轮发布 v26.410.20711 + v26.410.20710` › upgrade/manifest.json 最新版 sha256：6 个平台全部 MATCH |
| 12 | A. 首轮发布 v26.410.20711 + v26.410.20710 | core/manifest.json 的 latest | `A. 首轮发布 v26.410.20711 + v26.410.20710` › core/manifest.json 的 latest |
| 13 | A. 首轮发布 v26.410.20711 + v26.410.20710 | 旧 schema 兼容状态 OK（result=ok round=1 tags=2） | `A. 首轮发布 v26.410.20711 + v26.410.20710` › 旧 schema 兼容状态 OK |
| 14 | A. 首轮发布 v26.410.20711 + v26.410.20710 | 旧布局符号链接：core/<zip> → stable/<zip> | `A. 首轮发布 v26.410.20711 + v26.410.20710` › 旧布局符号链接：core/<zip> → stable/<zip> |
| 15 | A. 首轮发布 v26.410.20711 + v26.410.20710 | core/stable/manifest.json 别名 | `A. 首轮发布 v26.410.20711 + v26.410.20710` › core/stable/manifest.json 别名 |
| 16 | B. 只重下旧 tag v26.410.20710（模拟最新版指纹命中） | 旧 tag 被重下 | `B. 只重下旧 tag v26.410.20710（模拟最新版指纹命中）` › 旧 tag 被重下 |
| 17 | B. 只重下旧 tag v26.410.20710（模拟最新版指纹命中） | 最新版没有再下载（指纹命中） | `B. 只重下旧 tag v26.410.20710（模拟最新版指纹命中）` › 最新版没有再下载（指纹命中） |
| 18 | B. 只重下旧 tag v26.410.20710（模拟最新版指纹命中） | latest.json 版本 | `B. 只重下旧 tag v26.410.20710（模拟最新版指纹命中）` › latest.json 版本 |
| 19 | B. 只重下旧 tag v26.410.20710（模拟最新版指纹命中） | magisk_update.json 版本 | `B. 只重下旧 tag v26.410.20710（模拟最新版指纹命中）` › magisk_update.json 版本 |
| 20 | B. 只重下旧 tag v26.410.20710（模拟最新版指纹命中） | 未版本化资产 app-arm64-release.apk 仍是 26.410.20711 | `B. 只重下旧 tag v26.410.20710（模拟最新版指纹命中）` › 未版本化资产 app-arm64-release.apk 仍是 26.410.20711 |
| 21 | B. 只重下旧 tag v26.410.20710（模拟最新版指纹命中） | 未版本化资产 app-arm-release.apk 仍是 26.410.20711 | `B. 只重下旧 tag v26.410.20710（模拟最新版指纹命中）` › 未版本化资产 app-arm-release.apk 仍是 26.410.20711 |
| 22 | B. 只重下旧 tag v26.410.20710（模拟最新版指纹命中） | 未版本化资产 app-x86_64-release.apk 仍是 26.410.20711 | `B. 只重下旧 tag v26.410.20710（模拟最新版指纹命中）` › 未版本化资产 app-x86_64-release.apk 仍是 26.410.20711 |
| 23 | B. 只重下旧 tag v26.410.20710（模拟最新版指纹命中） | 未版本化资产 app-x86-release.apk 仍是 26.410.20711 | `B. 只重下旧 tag v26.410.20710（模拟最新版指纹命中）` › 未版本化资产 app-x86-release.apk 仍是 26.410.20711 |
| 24 | B. 只重下旧 tag v26.410.20710（模拟最新版指纹命中） | 未版本化资产 msi 仍是 26.410.20711 | `B. 只重下旧 tag v26.410.20710（模拟最新版指纹命中）` › 未版本化资产 msi 仍是 26.410.20711 |
| 25 | B. 只重下旧 tag v26.410.20710（模拟最新版指纹命中） | 未版本化资产 setup.exe 仍是 26.410.20711 | `B. 只重下旧 tag v26.410.20710（模拟最新版指纹命中）` › 未版本化资产 setup.exe 仍是 26.410.20711 |
| 26 | B. 只重下旧 tag v26.410.20710（模拟最新版指纹命中） | upgrade/manifest.json 最新版 sha256：26.410.20711: 6 个平台 sha256 全部 MATCH | `B. 只重下旧 tag v26.410.20710（模拟最新版指纹命中）` › upgrade/manifest.json 最新版 sha256：6 个平台全部 MATCH |
| 27 | B. 只重下旧 tag v26.410.20710（模拟最新版指纹命中） | core/manifest.json 的 latest 未回退 | `B. 只重下旧 tag v26.410.20710（模拟最新版指纹命中）` › core/manifest.json 的 latest 未回退 |
| 28 | B2. 无变化再跑一轮（不降级、不重下） | 没有重下任何 tag | `B2. 无变化再跑一轮（不降级、不重下）` › 没有重下任何 tag |
| 29 | B2. 无变化再跑一轮（不降级、不重下） | latest.json 版本 | `B2. 无变化再跑一轮（不降级、不重下）` › latest.json 版本 |
| 30 | B2. 无变化再跑一轮（不降级、不重下） | magisk_update.json 版本 | `B2. 无变化再跑一轮（不降级、不重下）` › magisk_update.json 版本 |
| 31 | B2. 无变化再跑一轮（不降级、不重下） | 未版本化资产 app-arm64-release.apk 仍是 26.410.20711 | `B2. 无变化再跑一轮（不降级、不重下）` › 未版本化资产 app-arm64-release.apk 仍是 26.410.20711 |
| 32 | B2. 无变化再跑一轮（不降级、不重下） | 未版本化资产 app-arm-release.apk 仍是 26.410.20711 | `B2. 无变化再跑一轮（不降级、不重下）` › 未版本化资产 app-arm-release.apk 仍是 26.410.20711 |
| 33 | B2. 无变化再跑一轮（不降级、不重下） | 未版本化资产 app-x86_64-release.apk 仍是 26.410.20711 | `B2. 无变化再跑一轮（不降级、不重下）` › 未版本化资产 app-x86_64-release.apk 仍是 26.410.20711 |
| 34 | B2. 无变化再跑一轮（不降级、不重下） | 未版本化资产 app-x86-release.apk 仍是 26.410.20711 | `B2. 无变化再跑一轮（不降级、不重下）` › 未版本化资产 app-x86-release.apk 仍是 26.410.20711 |
| 35 | B2. 无变化再跑一轮（不降级、不重下） | 未版本化资产 msi 仍是 26.410.20711 | `B2. 无变化再跑一轮（不降级、不重下）` › 未版本化资产 msi 仍是 26.410.20711 |
| 36 | B2. 无变化再跑一轮（不降级、不重下） | 未版本化资产 setup.exe 仍是 26.410.20711 | `B2. 无变化再跑一轮（不降级、不重下）` › 未版本化资产 setup.exe 仍是 26.410.20711 |
| 37 | B2. 无变化再跑一轮（不降级、不重下） | upgrade/manifest.json 最新版 sha256：26.410.20711: 6 个平台 sha256 全部 MATCH | `B2. 无变化再跑一轮（不降级、不重下）` › upgrade/manifest.json 最新版 sha256：6 个平台全部 MATCH |
| 38 | C. FORCE=1 全量重下 | 旧 tag 也重下了 | `C. FORCE=1 全量重下` › 旧 tag 也重下了 |
| 39 | C. FORCE=1 全量重下 | 最新版也重下了 | `C. FORCE=1 全量重下` › 最新版也重下了 |
| 40 | C. FORCE=1 全量重下 | latest.json 版本 | `C. FORCE=1 全量重下` › latest.json 版本 |
| 41 | C. FORCE=1 全量重下 | magisk_update.json 版本 | `C. FORCE=1 全量重下` › magisk_update.json 版本 |
| 42 | C. FORCE=1 全量重下 | 未版本化资产 app-arm64-release.apk 仍是 26.410.20711 | `C. FORCE=1 全量重下` › 未版本化资产 app-arm64-release.apk 仍是 26.410.20711 |
| 43 | C. FORCE=1 全量重下 | 未版本化资产 app-arm-release.apk 仍是 26.410.20711 | `C. FORCE=1 全量重下` › 未版本化资产 app-arm-release.apk 仍是 26.410.20711 |
| 44 | C. FORCE=1 全量重下 | 未版本化资产 app-x86_64-release.apk 仍是 26.410.20711 | `C. FORCE=1 全量重下` › 未版本化资产 app-x86_64-release.apk 仍是 26.410.20711 |
| 45 | C. FORCE=1 全量重下 | 未版本化资产 app-x86-release.apk 仍是 26.410.20711 | `C. FORCE=1 全量重下` › 未版本化资产 app-x86-release.apk 仍是 26.410.20711 |
| 46 | C. FORCE=1 全量重下 | 未版本化资产 msi 仍是 26.410.20711 | `C. FORCE=1 全量重下` › 未版本化资产 msi 仍是 26.410.20711 |
| 47 | C. FORCE=1 全量重下 | 未版本化资产 setup.exe 仍是 26.410.20711 | `C. FORCE=1 全量重下` › 未版本化资产 setup.exe 仍是 26.410.20711 |
| 48 | C. FORCE=1 全量重下 | upgrade/manifest.json 最新版 sha256：26.410.20711: 6 个平台 sha256 全部 MATCH | `C. FORCE=1 全量重下` › upgrade/manifest.json 最新版 sha256：6 个平台全部 MATCH |
| 49 | D. MAX_BYTES 预算裁剪（超限从最旧版本收缩） | 打印预算裁剪日志 | `D. MAX_BYTES 预算裁剪（超限从最旧版本收缩）` › 打印预算裁剪日志 |
| 50 | D. MAX_BYTES 预算裁剪（超限从最旧版本收缩） | 预算裁剪后只保留 1 个版本 | `D. MAX_BYTES 预算裁剪（超限从最旧版本收缩）` › 预算裁剪后只保留 1 个版本 |
| 51 | D. MAX_BYTES 预算裁剪（超限从最旧版本收缩） | 裁剪后 core latest 仍是最新 stable | `D. MAX_BYTES 预算裁剪（超限从最旧版本收缩）` › 裁剪后 core latest 仍是最新 stable |
| 52 | D. MAX_BYTES 预算裁剪（超限从最旧版本收缩） | latest.json 版本 | `D. MAX_BYTES 预算裁剪（超限从最旧版本收缩）` › latest.json 版本 |
| 53 | D. MAX_BYTES 预算裁剪（超限从最旧版本收缩） | magisk_update.json 版本 | `D. MAX_BYTES 预算裁剪（超限从最旧版本收缩）` › magisk_update.json 版本 |
| 54 | D. MAX_BYTES 预算裁剪（超限从最旧版本收缩） | 未版本化资产 app-arm64-release.apk 仍是 26.410.20712 | `D. MAX_BYTES 预算裁剪（超限从最旧版本收缩）` › 未版本化资产 app-arm64-release.apk 仍是 26.410.20712 |
| 55 | D. MAX_BYTES 预算裁剪（超限从最旧版本收缩） | 未版本化资产 app-arm-release.apk 仍是 26.410.20712 | `D. MAX_BYTES 预算裁剪（超限从最旧版本收缩）` › 未版本化资产 app-arm-release.apk 仍是 26.410.20712 |
| 56 | D. MAX_BYTES 预算裁剪（超限从最旧版本收缩） | 未版本化资产 app-x86_64-release.apk 仍是 26.410.20712 | `D. MAX_BYTES 预算裁剪（超限从最旧版本收缩）` › 未版本化资产 app-x86_64-release.apk 仍是 26.410.20712 |
| 57 | D. MAX_BYTES 预算裁剪（超限从最旧版本收缩） | 未版本化资产 app-x86-release.apk 仍是 26.410.20712 | `D. MAX_BYTES 预算裁剪（超限从最旧版本收缩）` › 未版本化资产 app-x86-release.apk 仍是 26.410.20712 |
| 58 | D. MAX_BYTES 预算裁剪（超限从最旧版本收缩） | 未版本化资产 msi 仍是 26.410.20712 | `D. MAX_BYTES 预算裁剪（超限从最旧版本收缩）` › 未版本化资产 msi 仍是 26.410.20712 |
| 59 | D. MAX_BYTES 预算裁剪（超限从最旧版本收缩） | 未版本化资产 setup.exe 仍是 26.410.20712 | `D. MAX_BYTES 预算裁剪（超限从最旧版本收缩）` › 未版本化资产 setup.exe 仍是 26.410.20712 |
| 60 | D. MAX_BYTES 预算裁剪（超限从最旧版本收缩） | upgrade/manifest.json 最新版 sha256：26.410.20712: 6 个平台 sha256 全部 MATCH | `D. MAX_BYTES 预算裁剪（超限从最旧版本收缩）` › upgrade/manifest.json 最新版 sha256：6 个平台全部 MATCH |
| 61 | D. MAX_BYTES 预算裁剪（超限从最旧版本收缩） | 旧 schema 兼容状态 OK（result=ok round=1 tags=1） | `D. MAX_BYTES 预算裁剪（超限从最旧版本收缩）` › 旧 schema 兼容状态 OK |
| 62 | D. MAX_BYTES 预算裁剪（超限从最旧版本收缩） | 单版本下载量等于单 tag 预算以内 | `D. MAX_BYTES 预算裁剪（超限从最旧版本收缩）` › 单版本下载量等于单 tag 预算以内 |

> 有一处故意改名以便去掉动态前缀：旧 `upgrade/manifest.json 最新版 sha256：<ver>: 6 个平台 sha256 全部 MATCH`
> → 新 `upgrade/manifest.json 最新版 sha256：6 个平台全部 MATCH`（版本号已在同一场景的 `latest.json 版本` 断言里校验）。
> **无缺项**：62 → 62。

除逐条搬移外，新增的护栏测试（不属于原 62 条）：

| 文件 | 覆盖 |
| --- | --- |
| `test/parity.test.ts` | 5 个场景的新旧 dist 树逐字节一致 |
| `test/entry.test.ts` | `--help`/未知参数/缺 tags-file 的退出码与「不落盘」、`--dry-run` 零写入、`MIRROR_STATUS*` 覆盖、`KEEP` 回落、空通道不建目录、nightly `.published-at` + 整目录替换、`VERIFY_SHA256` |
| `test/units.test.ts` | 合并先出现者为准/stage 排序/`--keep`/`--since`、平台裁剪、URL 通道化幂等、Python 兼容 JSON、状态双写（无 `channels`、round 递增）、预算裁剪边界 |
| `test/trigger.test.ts` | 鉴权、并发 409、状态回传、锁释放、外部持锁、超时终止、detail 脱敏 |

## 4. `/mirror/trigger` 设计

### 4.1 API

| 方法 | 路径 | 鉴权 | 响应 |
| --- | --- | --- | --- |
| `POST` | `/mirror/trigger` | `X-Mirror-Token` | `202 {ok:true, accepted:true, running:true, round, last_run}`；已在跑 → `409 {accepted:false, running:true, …}`；无/错 token → `401`；`Content-Length > 64 KiB` → `413` |
| `GET` | `/mirror/trigger` | `X-Mirror-Token` | `200 {ok:true, running, last_run, finished_at, result, detail, round, exit_code}` |
| `GET` | `/healthz` | 无 | `200 {ok:true, running, round}`（容器 liveness 用） |
| 其它 | — | — | `404` / `405`（JSON） |

`result` 取值：`ok` / `waiting`（本轮没有可发布内容或某通道半发布）/ `error`；`null` = 还没跑过。
`detail` 是同步输出最后 20 行（**已脱敏**、截断 4000 字符）。

```bash
curl -fsS -H "X-Mirror-Token: $T" https://dl.exvlan.era-x.cn/mirror/trigger | jq
curl -fsS -X POST -H "X-Mirror-Token: $T" https://dl.exvlan.era-x.cn/mirror/trigger | jq
```

### 4.2 鉴权

- token 只从 `MIRROR_TRIGGER_TOKEN_FILE`（推荐，`600`）或 `MIRROR_TRIGGER_TOKEN` 读；**缺失或 <16 字符直接拒绝启动**（退出码 2）。
- 比较用 `sha256` 摘要 + 逐字节定时安全比较；`401` 不区分「缺失/错误」，不回显任何片段。
- `GET` 也要 token：该端点经 Caddy 公网可达，状态里含版本号与日志尾巴，不公开。
- **禁止**把 token 写进仓库/`deploy.yaml` 明文/日志/URL 查询串；请求日志只记方法、路径、来源 IP。

### 4.3 防重入

1. 进程内 `child !== null` 判定（单飞，不排队 —— 发版触发与定时同步不叠加）。
2. 锁文件 `MIRROR_TRIGGER_LOCK`（默认 `$TMPDIR/mirror-sync.lock`）：`open(..., 'wx')` + 写入 pid。
3. 锁被**他人**持有（例如宿主 systemd timer 用同一路径）→ `409`，不抢跑；超过 `MIRROR_TRIGGER_TIMEOUT_SEC` 视为崩溃遗留并接管。
4. 子进程退出/异常/超时都会释放锁；`MIRROR_SYNC_LOCK=''` 传给子进程（锁由服务进程持有）。

### 4.4 超时与周期

| 变量 | 默认 | 作用 |
| --- | --- | --- |
| `MIRROR_TRIGGER_TIMEOUT_SEC` | `2700` | 单轮上限：到点 `SIGTERM`，10s 后 `SIGKILL`，`result=error` + detail 注明超时 |
| `MIRROR_TRIGGER_INTERVAL_SEC` | `900` | 周期同步；`0` = 只按需触发（容器里用它同时承担定时与按需，宿主 systemd 部署时置 `0`，定时交给 `.timer`） |
| `MIRROR_TRIGGER_PORT` / `_HOST` / `_PATH` | `8899` / `127.0.0.1` / `/mirror/trigger` | 监听参数（只监听回环，由 Caddy 反代） |
| `MIRROR_TRIGGER_STATE` | `$TMPDIR/mirror-trigger-state.json` | 状态持久化（容器重启后 `round` 连续） |
| `MIRROR_SYNC_ENTRY` | 同目录 `index.ts` | 同步入口 |

启动即跑一轮（与旧容器「起来就同步」一致）。`result` 优先采用同步自己写的兼容状态（能区分 `waiting`），否则按退出码。

## 5. 部署改动建议（**本轮未执行**；发布机操作由 owner 执行）

### 5.1 `~/deploy/deploy.yaml`：`exvlan-mirror` 容器

现状：`alpine` + `apk add github-cli python3 bash coreutils`，启动时从仓库拉 5 个文件，再用 shell 循环跑同步。
目标：`oven/bun:alpine`，启动时拉 `index.ts` + `src/*.ts` + `trigger-server.ts`，由 trigger 服务同时承担「按需触发 + 周期同步」。

```diff
     - name: exvlan-mirror
       # exvlan 资产端：/core 内核资产 + /upgrade 应用/Magisk 更新（详见 docs/rules/workflow/mirror-deploy.md）
-      image: docker.io/alpine:latest
+      # Bun 单一真源：同步脚本与 trigger 服务都是 TypeScript（docs/mirror-toolchain.md）
+      # 建议钉 minor（oven/bun:1.4-alpine），避免 upstream 大版本漂移；此处按约定用 :alpine
+      image: docker.io/oven/bun:alpine
       imagePullPolicy: IfNotPresent
       env:
         - name: TZ
           value: Asia/Shanghai
         - name: REPO
           value: exvlan/exvlan
@@
         - name: TMPDIR
           value: /srv/exvlan/.work
         # 同步脚本基线：默认取最新 stable tag；这里的 stable tag 尚未包含通道化脚本，
         # 先钉在含通道化改造的分支上，等 stable release 带上 channelize_urls.py 后删掉本项即可。
         - name: MIRROR_REF
           value: develop
+        # trigger 服务：只监听回环，由 Caddy 反代 /mirror/trigger（下方 Caddy 一键 diff）
+        - name: MIRROR_TRIGGER_HOST
+          value: 127.0.0.1
+        - name: MIRROR_TRIGGER_PORT
+          value: "8899"
+        # 令牌只从文件读（600，宿主 infra/exvlan/mirror-trigger-token）
+        - name: MIRROR_TRIGGER_TOKEN_FILE
+          value: /etc/exvlan-mirror/trigger-token
+        # 容器里由 trigger 服务承担周期同步（等价旧 while+sleep 900 循环）
+        - name: MIRROR_TRIGGER_INTERVAL_SEC
+          value: "900"
+        - name: MIRROR_TRIGGER_TIMEOUT_SEC
+          value: "2700"
+        - name: MIRROR_SYNC_ENTRY
+          value: /app/scripts/exvlan-mirror/index.ts
       command:
         - sh
         - -c
         - |
-          echo "https://dl-cdn.alpinelinux.org/alpine/edge/community" >> /etc/apk/repositories
-          if ! apk add --no-cache github-cli python3 bash coreutils; then
-            echo "[mirror] ❌ 依赖安装失败（github-cli/python3/bash/coreutils），退出以便可见" >&2
-            exit 1
-          fi
-          # 启动前：把镜像同步脚本刷成仓库基线（失败只告警，沿用现有脚本，不阻断）
-          REF="${MIRROR_REF:-$(gh api repos/exvlan/exvlan/releases/latest --jq .tag_name 2>/dev/null || true)}"
-          if [ -n "$REF" ]; then
-            echo "[mirror] 同步脚本基线：ref=$REF"
-            for f in exvlan-kernel-sync.sh merge_manifests.py filter_manifest_platforms.py channelize_urls.py mirror_status.py; do
-              dst="/app/scripts/exvlan-mirror/$f"
-              if body="$(gh api "repos/exvlan/exvlan/contents/script/kernel-mirror/$f?ref=$REF" --jq .content 2>/dev/null)" \
-                && [ -n "$body" ] \
-                && printf '%s' "$body" | base64 -d > "$dst.new" 2>/dev/null \
-                && [ -s "$dst.new" ]; then
-                chmod 0644 "$dst.new"
-                case "$f" in *.sh) chmod 0755 "$dst.new" ;; esac
-                mv -f "$dst.new" "$dst"
-                echo "[mirror] 已更新 $f"
-              else
-                rm -f "$dst.new"
-                echo "[mirror] 警告：拉取 $f 失败（ref=$REF），沿用现有脚本" >&2
-              fi
-            done
-          else
-            echo "[mirror] 拉取脚本失败（无 stable tag / 网络），沿用现有脚本" >&2
-          fi
-          mkdir -p /srv/exvlan/.work
-          touch /tmp/mirror-heartbeat
-          delay=300
-          while :; do
-            /app/scripts/exvlan-mirror/exvlan-kernel-sync.sh
-            rc=$?
-            touch /tmp/mirror-heartbeat
-            if [ "$rc" -eq 0 ]; then
-              delay=300
-              sleep 900
-            elif [ "$rc" -eq 3 ]; then
-              sleep 300
-            else
-              sleep "$delay"
-              case "$delay" in
-                300) delay=600 ;;
-                600) delay=1200 ;;
-                *) delay=1800 ;;
-              esac
-            fi
-          done
+          echo "https://dl-cdn.alpinelinux.org/alpine/edge/community" >> /etc/apk/repositories
+          # 仍然需要 gh CLI：元数据与 release download 都走 gh（token 语义见 docs/mirror-toolchain.md）
+          if ! apk add --no-cache github-cli; then
+            echo "[mirror] ❌ 依赖安装失败（github-cli），退出以便可见" >&2
+            exit 1
+          fi
+          # 启动前把 TS 工具链刷成仓库基线（失败只告警，沿用现有脚本，不阻断）
+          REF="${MIRROR_REF:-$(gh api repos/exvlan/exvlan/releases/latest --jq .tag_name 2>/dev/null || true)}"
+          if [ -n "$REF" ]; then
+            echo "[mirror] 同步脚本基线：ref=$REF"
+            files="index.ts trigger-server.ts src/config.ts src/log.ts src/pyjson.ts src/version.ts \
+          src/tags.ts src/gh.ts src/manifest.ts src/channelize.ts src/fsx.ts src/publish.ts \
+          src/status.ts src/run.ts"
+            for f in $files; do
+              dst="/app/scripts/exvlan-mirror/$f"
+              mkdir -p "$(dirname "$dst")"
+              if body="$(gh api "repos/exvlan/exvlan/contents/script/kernel-mirror/$f?ref=$REF" --jq .content 2>/dev/null)" \
+                && [ -n "$body" ] \
+                && printf '%s' "$body" | base64 -d > "$dst.new" 2>/dev/null \
+                && [ -s "$dst.new" ]; then
+                chmod 0644 "$dst.new"
+                mv -f "$dst.new" "$dst"
+                echo "[mirror] 已更新 $f"
+              else
+                rm -f "$dst.new"
+                echo "[mirror] 警告：拉取 $f 失败（ref=$REF），沿用现有文件" >&2
+              fi
+            done
+          else
+            echo "[mirror] 拉取脚本失败（无 stable tag / 网络），沿用现有脚本" >&2
+          fi
+          mkdir -p /srv/exvlan/.work
+          # 前台运行：trigger 服务负责周期同步（MIRROR_TRIGGER_INTERVAL_SEC）+ 按需触发
+          exec bun /app/scripts/exvlan-mirror/trigger-server.ts
       volumeMounts:
         - name: deploy
           mountPath: /app
         - name: gh-config
           mountPath: /root/.config/gh
           readOnly: true
         - name: exvlan-dist
           mountPath: /srv/exvlan
+        - name: mirror-trigger-token
+          mountPath: /etc/exvlan-mirror/trigger-token
+          readOnly: true
       livenessProbe:
         exec:
           command:
-            - sh
-            - -c
-            - find /tmp/mirror-heartbeat -mmin -60 -print -quit | grep -q .
+            - wget
+            - -q
+            - -O
+            - /dev/null
+            - http://127.0.0.1:8899/healthz
         initialDelaySeconds: 30
         periodSeconds: 60
         timeoutSeconds: 10
         failureThreshold: 3
       resources:
         limits:
           memory: "256Mi"
```

并在 `volumes:` 段新增（宿主文件先落盘，`chmod 600`）：

```diff
     - name: exvlan-dist
       hostPath:
         path: /root/deploy/assets/exvlan-dist
+    # trigger 令牌（600）。不要写进 deploy.yaml / 仓库 / git
+    - name: mirror-trigger-token
+      hostPath:
+        path: /root/deploy/infra/exvlan/mirror-trigger-token
+        type: File
```

生成令牌（在 `sg` 上执行，不要在本地仓库里留任何副本）：

```bash
umask 077
head -c 32 /dev/urandom | base64 | tr -d '\n/' > ~/deploy/infra/exvlan/mirror-trigger-token
chmod 600 ~/deploy/infra/exvlan/mirror-trigger-token
```

> 注意：token 只出现在「宿主文件」与「GitHub secret」两处，两边手动对齐即可。

### 5.2 Caddy（`~/deploy/infra/caddy/Caddyfile` 的 `{$DL_DOMAIN}` 段）

`pod` 是 `hostNetwork: true`，所以 Caddy 容器可以用 `127.0.0.1:8899` 直连 mirror 容器的 trigger 服务。

```diff
 	handle /upgrade/* {
 		file_server
 	}
 
+	# 镜像同步触发端点（docs/mirror-toolchain.md）：
+	#   POST → 立刻同步（Header X-Mirror-Token），GET → 最近一次状态
+	# handle 按声明顺序匹配，必须在最后的 respond 404 之前。
+	handle /mirror/trigger {
+		request_body {
+			max_size 16KB
+		}
+		reverse_proxy 127.0.0.1:8899 {
+			header_up X-Real-IP {remote_host}
+			header_down -Server
+			transport http {
+				dial_timeout 5s
+				response_header_timeout 30s
+			}
+		}
+	}
+
 	respond 404
```

`script/kernel-mirror/caddy.Caddyfile`（仓库里的完整站点文件）已包含同一段，可用于 `caddy validate` 预检。
不要给 `/mirror/trigger` 加 `@manifest`/immutable 缓存头，也不要把它放进 `log` 的 `format` 里输出请求头。

### 5.3 非容器部署（systemd）

宿主直跑时用新增的 `exvlan-kernel-mirror-trigger.service`（`MIRROR_TRIGGER_INTERVAL_SEC=0`，定时仍由
`exvlan-kernel-mirror.timer` 负责），安装方式见 §7.1。

## 6. CI 触发点与超时策略

`.github/workflows/pipeline.yml` 新增 job（位置：`release` / `nightly_release` 之后、`notify` 之前）：

```yaml
  mirror_sync:
    name: Trigger Mirror Sync
    needs: [release, nightly_release]
    if: >-
      always() && !cancelled() &&
      (needs.release.result == 'success' || needs.nightly_release.result == 'success')
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7
        with:
          sparse-checkout: script/ci/mirror-trigger.sh
          sparse-checkout-cone-mode: false
      - name: Trigger mirror sync
        env:
          MIRROR_TRIGGER_URL: ${{ vars.MIRROR_TRIGGER_URL }}
          MIRROR_TRIGGER_TOKEN: ${{ secrets.MIRROR_TRIGGER_TOKEN }}
        run: bash script/ci/mirror-trigger.sh
```

为什么放这里：最小侵入（复用已在跑的 job 结果，不改 `release.yml` 的 reusable workflow 契约）；
`release` 成功才可能拉到新资产；nightly 走同一入口。

`script/ci/mirror-trigger.sh` 的策略：

| 情况 | 行为 |
| --- | --- |
| 未配置 URL/token | `::notice::` 跳过，exit 0 |
| `POST` → 202 | 轮询 `GET`，直到 `running=false && round >= 触发时 round` |
| `POST` → 409 | 视为「镜像机已在跑」，同样轮询等待完成 |
| 401 / 网络失败 / 5xx | `::warning::` 并 exit 0（回退到定时同步） |
| 轮询超过 `MIRROR_TRIGGER_WAIT`（默认 **120s**） | `::warning::` 并 exit 0，**不阻塞发版** |
| 同步结束但 `result != ok` | `::warning::`（镜像机保留旧内容，下一轮重试） |

配置位置：repo **variable** `MIRROR_TRIGGER_URL=https://dl.exvlan.era-x.cn/mirror/trigger`，
repo **secret** `MIRROR_TRIGGER_TOKEN`（与宿主 `mirror-trigger-token` 内容一致）。

## 7. 部署到 `sg` 的操作步骤与回滚（供 owner 执行）

### 7.1 步骤

```bash
# 0) 备份（deploy 仓已有 bak-* 习惯，沿用）
ssh sg 'cp ~/deploy/deploy.yaml ~/deploy/deploy.yaml.bak-$(date +%Y%m%d-%H%M%S)'
ssh sg 'cp ~/deploy/infra/caddy/Caddyfile ~/deploy/infra/caddy/Caddyfile.bak-$(date +%Y%m%d-%H%M%S)'

# 1) 令牌（600）
ssh sg 'umask 077; mkdir -p ~/deploy/infra/exvlan; head -c 32 /dev/urandom | base64 | tr -d "\n/" > ~/deploy/infra/exvlan/mirror-trigger-token; chmod 600 ~/deploy/infra/exvlan/mirror-trigger-token'

# 2) Caddy：先 validate 再 reload（容器内 caddy 版本自带 validate）
ssh sg 'podman exec <caddy-container> caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile'
ssh sg 'podman exec <caddy-container> caddy reload --config /etc/caddy/Caddyfile --adapter caddyfile'

# 3) deploy.yaml：按 §5.1 改 exvlan-mirror 容器 + volumes
#    ⚠️ 改 env 必须重建 pod：`podman play kube` 不能靠 restart 生效
ssh sg 'cd ~/deploy && podman play kube deploy.yaml'
#    （若同 pod 已有容器，先停掉旧 pod 再 play：podman pod stop/rm <pod>，仅限该 pod）

# 4) 验证
ssh sg 'podman logs --tail 50 <mirror-container>'      # 应见 [trigger] 监听 … /mirror/trigger
T=$(ssh sg 'cat ~/deploy/infra/exvlan/mirror-trigger-token')
curl -fsS -H "X-Mirror-Token: $T" https://dl.exvlan.era-x.cn/mirror/trigger | jq '.running,.result,.round'
curl -fsS -X POST -H "X-Mirror-Token: $T" https://dl.exvlan.era-x.cn/mirror/trigger | jq
sleep 20
curl -fsS -H "X-Mirror-Token: $T" https://dl.exvlan.era-x.cn/mirror/trigger | jq '.running,.result,.detail'

# 5) 资产未受影响（通道目录/别名/状态都还在）
curl -fsS https://dl.exvlan.era-x.cn/core/manifest.json | jq '.latest,(.versions|length)'
curl -fsS https://dl.exvlan.era-x.cn/core/mirror-status.json | jq '.channels|keys'
ls -l /root/deploy/assets/exvlan-dist/core/manifest.json   # → stable/manifest.json

# 6) GitHub：variable MIRROR_TRIGGER_URL + secret MIRROR_TRIGGER_TOKEN
gh variable set MIRROR_TRIGGER_URL --body 'https://dl.exvlan.era-x.cn/mirror/trigger'
gh secret set MIRROR_TRIGGER_TOKEN --body "$T"
```

### 7.2 回滚

```bash
# 1) 容器回旧镜像/旧脚本：还原 deploy.yaml 再重建（不要 podman restart）
ssh sg 'cd ~/deploy && cp deploy.yaml.bak-<ts> deploy.yaml && podman play kube deploy.yaml'

# 2) Caddy 回旧配置
ssh sg 'cp ~/deploy/infra/caddy/Caddyfile.bak-<ts> ~/deploy/infra/caddy/Caddyfile'
ssh sg 'podman exec <caddy-container> caddy reload --config /etc/caddy/Caddyfile --adapter caddyfile'

# 3) CI 侧不再触发（可选）：删掉 variable/secret 即自动跳过（脚本 exit 0）
gh variable delete MIRROR_TRIGGER_URL || true
gh secret delete MIRROR_TRIGGER_TOKEN || true
```

回滚安全性：镜像目录（`assets/exvlan-dist`）只由同步脚本写，旧脚本读同一份目录与 `.sync-state`，
**互不影响**；`trigger-token` 文件留着也无害（没有进程读它）。

## 8. 后续（本轮未做）

- 一个周期后删除 6 个 legacy 文件（`exvlan-kernel-sync.sh` / `merge_manifests.py` /
  `filter_manifest_platforms.py` / `channelize_urls.py` / `mirror_status.py` / `e2e-sync-test.sh`），
  同时删掉 `deploy.yaml` 里 `MIRROR_REF=develop` 的临时钉版。
- `script/ci/set-version.sh` 的夹具白名单已覆盖 `script/kernel-mirror/**`，新测试夹具沿用 20710→20712 版本阶梯，无需额外改动。
