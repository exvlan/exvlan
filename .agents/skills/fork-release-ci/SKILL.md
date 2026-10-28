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

## 六个必知坑

1. **reusable `permissions` 必须是调用方子集**：被调 workflow 声明的权限超过调用方授予 → 整轮 `startup_failure`（无日志）。解法：在 `pipeline.yml` 顶层授予被调方所需权限（如 `pull-requests: read`）。
2. **发版产物用 Actions artifact，不用 `actions/cache`**：cache 有 10GB/仓库配额，**LRU 驱逐**会使最后运行的 release 取不到较早保存的产物；改用 `upload-artifact`（各组件，带前缀如 `core-*`/`studio-*`/`mobile-*`/`magisk`）+ release 端 `download-artifact` 的 **`pattern`** 精确拉取。
   - ⚠️ 不要用“全量 download-artifact”：docker buildx 会产生名为 `<owner>~<repo>~*.dockerbuild` 的 artifact，**名字含 `~`（保留分隔符）→ 解压失败**。
3. **同提交去重**：`concurrency.group` 用 `github.sha`（非 `github.ref`）；发版**先 push develop 再 push tag** → 同 sha 下 tag 运行胜出、分支运行被 `cancel-in-progress` 取消，只跑一轮。
4. **`productName` 决定 Windows MSI `UpgradeCode`**：改动会使其变成“并存新装”而非升级。升级链要连续就别轻改 `productName`（`tauri.conf.json`）。
5. **可见性开关**：私仓 Actions 额度耗尽时 runner 分配失败（job `steps=0`、秒失败）→ 临时 `visibility=public` 跑 CI，**完毕立即改回 private**。
6. **预提交格式化器会重写文件语法 → 打挂 CI 解析**：Vite+ 的 `vp check --fix`（`.vite-hooks` 预提交，配置在 `vite.config.ts` 的 `staged`）会按 oxfmt 规则重排；`.json5` 会被**反引用化**（`"version"` → `version`、补尾逗号）→ 变成**非法 JSON**，于是任何用 `jq` 解析它的步骤会炸（实例：`ohos.yml` 用 `jq` 改写 `oh-package.json5` → `jq: parse error: Invalid numeric literal at line 2, column 10`）。
   - 规避：`vite.config.ts` → `fmt: { ignorePatterns: ['**/*.json5'] }`（已在本项目落地）。
   - 判据：**凡 CI 会用 `jq`/`JSON.parse` 读的文件**（配置/清单/清单类 JSON），被格式化器改过后，本地必须复跑一次解析（`jq -e . <file>`）再提交；CI 侧出现 `parse error` 时，先 `git diff` 该文件确认是不是被格式化器改写了语法。

## 发版流程

1. 版本对齐（见 `upstream-fork-sync` / 版本规范）。
2. 提交落 `feature` → 压 **squash** 到 `develop`（保持 `develop` 线性、0 merge）。
3. 移动 tag：`git tag -f -a vX && git push -f origin vX`。
4. tag push 触发全链 + `release`（自动 `draft:false` 发布）+ `publish_*`。
5. `release.needs` 含 `publish_*` → **发布失败即阻塞 release**。
6. 校验资产命名/正文/包，随后恢复 private。

### 发布后修缺陷：移动 tag 重跑（而非新开版本）

若 tag 的构建有缺陷（如某旁路作业红）而 release 已发布，**先把修复落 develop，再重定位同一 tag**，靠幂等把重跑变绿——不要为 CI 单点问题额外发一个版本：

```bash
git tag -d vX && git tag -a vX -m "..." <new-develop-head>
git push origin refs/tags/vX --force
```

前提（否则重跑会红）：
- **发布作业必须幂等**：`publish-go` 已内置「tag 存在则跳过」；`publish-npm` 需自检 `npm view @<owner>/<pkg>@<ver>` 已存在则跳过（否则 npm 409）。
- `release` 用 `softprops/action-gh-release` 覆盖同名资产 → 重跑安全。
- tag 刚发布、无外部消费者时再这么做；已广泛消费的版本应改为**发新补丁版**。

⚠️ **Go module tag 例外**：`exvlan/go/<ver>` 经 module proxy/sumdb 缓存后**不可变**，不要为对齐而移动；如需与 release tag 同提交，应在**首次发布前**就对齐。

## 包发布

- **npm → GitHub Packages**：`setup-node` 设 `registry-url: https://npm.pkg.github.com` + `scope: '@<owner>'`；各包 `publishConfig.registry=https://npm.pkg.github.com`（不依赖环境 `.npmrc`）；`pnpm -r publish --access public`；`NODE_AUTH_TOKEN=${{ secrets.GITHUB_TOKEN }}`。
  - **务必做幂等**：发布前 `npm view @<owner>/<pkg>@<version> --registry https://npm.pkg.github.com` 命中即 `exit 0` 跳过；否则重跑 tag 会以 409「cannot publish over previously published version」失败，进而阻塞 `release`。
- **Go → git tag**：module 在子目录时 tag 前缀必须是子目录路径（如 `exvlan/go/vX.Y.Z`）；私仓消费者需 `GOPRIVATE=github.com/<owner>/*`。
- 发布作业需 `permissions: packages: write`（npm）/ `contents: write`（推 tag）。

## 依赖审计门禁

- **workflow**：`.github/workflows/audit.yml`（只提供 `workflow_call`/`workflow_dispatch`），由 `pipeline.yml` 的 `audit` job 调用。
  - 门控：tag/dispatch **全量**、PR **全量**、push 按 `deps` 路径过滤（`Cargo.*`、`**/package.json`、`**/pnpm-lock.yaml`、`**/go.mod(,sum)`、`.cargo/audit.toml`、`audit.yml`）。
- **Rust**：`cargo audit`；允许清单 = `.cargo/audit.toml` 的 `[advisories].ignore`。
  - ⚠️ 文件名：`cargo-audit 0.22` 读 **`.cargo/audit.toml`**；根目录 `audit.toml` **不生效**（踩过）。
  - 元数据放 `.cargo/audit-allowlist.toml`（`id`/`review_by`/`reason`），由 `script/check_audit_allowlist.py` 校验：**过期即失败**、且 id 集合必须与 `audit.toml` 一致 —— 防止允许清单变成永久豁免。
- **npm**：`pnpm audit --prod`（root 与 `packages/sdk` 各一次），只阻塞**生产依赖**；dev/build 链交给 Dependabot + issue 跟踪。
  - CI 无 `.npmrc` → 默认 npmjs；显式 `--registry https://registry.npmjs.org`（本地镜像如 npmmirror **没有 audit 端点**，会 `ERR_PNPM_AUDIT_ENDPOINT_NOT_EXISTS`）。
  - root lock 未跟踪时需先 `pnpm install --ignore-scripts --no-frozen-lockfile` 才有锁可审。
- **build-dep 陷阱**：`cargo audit` 扫 Cargo.lock **全量**，包括 `target = "cfg(windows)"` 的 build-dependencies。实例：`zip`（`exvlan-proto` 的 Windows-only build-dep）曾带 **9.1 critical**（RUSTSEC-2026-0369，`extract()` 经 symlink 越界），本地 advisory-db 可能滞后而 CI 不滞后 → **以 CI 为准**，该类大版本（8→9）该升就升。

## 依赖大版本升级（`deps:major` / taze）

`pnpm deps:major` = `bunx taze major -r --peer -w`。**不要直接 `-w` 盲升**，先 `taze major -r --peer`（不带 `-w`）看影响面。已知坑：

1. **taze 会写 tag（`@v7`），破坏 SHA 钉法**：升完必须重跑钉 SHA 脚本（本会话的 `pin_actions.py` 等价物），确认「残留未钉 = 0」。
2. **`vite: catalog:` 解析失败**：taze 不懂 vite-plus 的 catalog（会报 `Timeout requesting "vite"`）。保持 catalog 不动，手动跳过 vite。
3. **TS 7 × vue-tsc 不兼容**：`vue-tsc 3.3.13` 依赖 `typescript/lib/tsc.js`，TS 7（Go 端口）已移除该导出 → `ERR_PACKAGE_PATH_NOT_EXPORTED`。TypeScript 需留在 5.x，或等 vue-tsc 支持。
4. **workspace 同库多版本**：例如同时存在 `primevue@4`(studio/ui/generator) 与 `primevue@5`(std) → `@primeuix/styles` 2.x/3.x 混装，表现为 **vitest 报 `does not provide an export named 'preflight'`**（生产构建却正常）。升级同一 UI 库必须**四包一起升**并用 `pnpm why -r <pkg>` 确认只剩一个版本。
5. **PrimeVue 5 重命名**：`InputSwitch → ToggleSwitch`、主题 token `inverseColor → contrastColor`。漏改 `ui` 会导致 `prepare-build`（它构建前端）失败，而不是只有前端作业红。
6. **`ubuntu-latest` 已迁 Ubuntu 26.04**：`android-actions/setup-android` 的 `cmdline-tools-version` 必须写**长版本（build）**（20.0 → `14742923`）；写短版本（`12.0`）会被拼成 `commandlinetools-linux-12_latest.zip` → **404**。
7. **pnpm 版本要统一**：根 `devEngines.packageManager.version`、`packages/studio.packageManager`、CI 各处 `pnpm/action-setup.version` 必须一致；pnpm 12 会按 `devEngines` 回落，混用会导致「用 A 版本执行、实际跑 B 版本」。
8. **每批必过的本地验证**：`pnpm install`（锁不变）→ 受影响的 `vue-tsc -b`/`vp build` → 单测（std `test:config-ui`、studio `test:mobile-vpn`）→ `cargo check --locked`。

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
