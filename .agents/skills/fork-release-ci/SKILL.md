---
name: fork-release-ci
description: Fork 的发布与 CI 运维：单入口 pipeline 编排与门控、发版 tag→构建→Release、GitHub Packages(npm)/Go module 发布、临时 public 跑 CI、清理失败运行。用于发版、CI 红/绿排查、runner 不可用、发布失败、清理 Actions 历史。触发词：发版、release、打 tag 发版、CI 统一编排、pipeline、清理失败 ci、actions 清理、发布 sdk/go、npm 发布、临时 public。
---

# fork-release-ci

维护 fork 的**发布与 CI 编排**（发版、编排、发布、清理）。核心经验：**单一入口编排 + 产物 artifact 化 + 权限子集 + 同提交去重 + 可见性开关**。

## 何时使用

- 要发版（打 tag → CI 构建 → GitHub Release + 包发布）。
- CI 失败排查（`startup_failure`、取不到产物、docker/发布节点红）。
- 清理 Actions 历史、切换仓库可见性以跑 CI。

## 单一事实源

- 编排：`.github/workflows/pipeline.yml`（唯一 push/PR/tag 入口）。
- 发版：`release.yml`；发布：`publish-npm.yml` / `publish-go.yml`；组件：`test/core/studio/mobile/docker/go/js/nix/ohos.yml`。
- 约束：`AGENTS.md` 的《CI 编排（统一 pipeline 单入口）》。

## 编排与门控（推荐形态）

- **所有组件只保留 `workflow_call`(+`workflow_dispatch`)**，不得自带 `on: push/pull_request/tags`；由 `pipeline.yml` 统一编排。
- **门控矩阵**：
  | 事件 | tests | core | studio/mobile/docker | go/js/nix/ohos | release |
  |---|---|---|---|---|---|
  | `feature` push | ✓ | ✓（轻量，如仅 x86_64、无 magisk） | ✗ | ✗ | ✗ |
  | PR | ✓ | ✓ | ✗ | ✗ | ✗ |
  | `develop`/`main` push | ✓ | ✓ | ✓ | 按路径门控 | ✗ |
  | tag `v*` / `workflow_dispatch` | ✓ | ✓ | ✓ | **全量**（绕门控） | ✓(仅 tag) |
- **不因内容重复跳过**中游构建（`core/test/mobile` 的 `should_skip` 仅由 PR 路径门控决定）——否则下游（docker/release）取不到本轮产物。

## 五个必知坑

1. **reusable `permissions` 必须是调用方子集**：被调 workflow 声明的权限超过调用方授予 → 整轮 `startup_failure`（无日志）。解法：在 `pipeline.yml` 顶层授予被调方所需权限（如 `pull-requests: read`）。
2. **发版产物用 Actions artifact，不用 `actions/cache`**：cache 有 10GB/仓库配额，**LRU 驱逐**会使最后运行的 release 取不到较早保存的产物；改用 `upload-artifact`（各组件，带前缀如 `core-*`/`studio-*`/`mobile-*`/`magisk`）+ release 端 `download-artifact` 的 **`pattern`** 精确拉取。
   - ⚠️ 不要用“全量 download-artifact”：docker buildx 会产生名为 `<owner>~<repo>~*.dockerbuild` 的 artifact，**名字含 `~`（保留分隔符）→ 解压失败**。
3. **同提交去重**：`concurrency.group` 用 `github.sha`（非 `github.ref`）；发版**先 push develop 再 push tag** → 同 sha 下 tag 运行胜出、分支运行被 `cancel-in-progress` 取消，只跑一轮。
4. **`productName` 决定 Windows MSI `UpgradeCode`**：改动会使其变成“并存新装”而非升级。升级链要连续就别轻改 `productName`（`tauri.conf.json`）。
5. **可见性开关**：私仓 Actions 额度耗尽时 runner 分配失败（job `steps=0`、秒失败）→ 临时 `visibility=public` 跑 CI，**完毕立即改回 private**。

## 发版流程

1. 版本对齐（见 `upstream-fork-sync` / 版本规范）。
2. 提交落 `feature` → 压 **squash** 到 `develop`（保持 `develop` 线性、0 merge）。
3. 移动 tag：`git tag -f -a vX && git push -f origin vX`。
4. tag push 触发全链 + `release`（自动 `draft:false` 发布）+ `publish_*`。
5. `release.needs` 含 `publish_*` → **发布失败即阻塞 release**。
6. 校验资产命名/正文/包，随后恢复 private。

## 包发布

- **npm → GitHub Packages**：`setup-node` 设 `registry-url: https://npm.pkg.github.com` + `scope: '@<owner>'`；各包 `publishConfig.registry=https://npm.pkg.github.com`（不依赖环境 `.npmrc`）；`pnpm -r publish --access public`；`NODE_AUTH_TOKEN=${{ secrets.GITHUB_TOKEN }}`。
- **Go → git tag**：module 在子目录时 tag 前缀必须是子目录路径（如 `exvlan/go/vX.Y.Z`）；私仓消费者需 `GOPRIVATE=github.com/<owner>/*`。
- 发布作业需 `permissions: packages: write`（npm）/ `contents: write`（推 tag）。

## 清理失败运行

```bash
bash .agents/skills/fork-release-ci/scripts/clean_runs.sh            # 删所有非 success
bash .agents/skills/fork-release-ci/scripts/clean_runs.sh --failed   # 仅删 failure
```

（GitHub 无批量删除；脚本走 REST 循环。`gh` 默认仓库可能不是本仓 → 脚本显式 `-R`。）

## 工具脚本

- `scripts/clean_runs.sh [--failed]`：删除非 success（或仅 failure）的 workflow run。
- `scripts/verify_release_assets.sh <tag>`：校验 Release 为 published、资产齐全（按平台矩阵）、正文无禁用字样、npm/go 包存在。
- `scripts/toggle_visibility.sh public|private`：切换仓库可见性（跑 CI 前后用）。

## 红线

- 发版产物**不要**依赖 `actions/cache`（配额驱逐）。
- **不要**用全量 `download-artifact`（buildx `~` 名会炸）。
- 改动仓库可见性后**务必**恢复。
- 需临时的 public 仅用于跑 CI，不改变代码可见性策略。
