# Exvlan 项目约束

## 分支合并策略

- **上游只进 feature**：`develop => feature`（develop 同步进 feature）；`upstream/releases/v.* => feature`（上游 Exvlan releases 只合入 feature）。
- **feature => (squash) develop**：feature 以 **squash** 方式并入 develop。
- **develop 保持独立**：develop 不允许混入任何上游链（上游 = Exvlan 等外部仓库），其历史应始终为：本地基点（如 `382534d0` / `9779923b`）→ 单个 squash 提交。
- 禁止对 develop 直接 merge 上游分支。

**历史背景**：早期为 `dev/feature` + `dev/squash` 双分支互 merge 并直连上游 release（旧引用保留于 `refs/original/`）；
`b2c96192` 定格现行「develop 独立 + 上游只进 feature」模型。

**操作命令**（完整流程与冲突策略见 [`docs/rules/workflow/upstream-sync.md`](./docs/rules/workflow/upstream-sync.md)）：

```bash
# A. 上游 release → feature（merge，非 rebase）
git fetch upstream --tags --prune && git checkout feature && git merge --no-ff upstream/releases/vX.Y.Z

# B. develop → feature；内容一致时用 -s ours 仅建立祖先关系
git fetch origin && git merge origin/develop
git merge -s ours origin/develop --no-edit   # 需要时

# C. feature →（squash）develop
git checkout develop && git merge --squash feature
git commit -m "feat: <批次描述>（压缩 <a>..<b>）"
```

**不变量校验**：`git rev-list --merges --count develop` 必须为 `0`；
`git merge-base --is-ancestor upstream/main develop` 必须非祖先。

## CI 编排（统一 pipeline 单入口）

- **唯一入口**：`.github/workflows/pipeline.yml`。**所有** CI 都经其依赖链编排；其余工作流（`test/core/studio/mobile/docker/release/go/js/nix/ohos`）一律**只保留 `workflow_call`(+`workflow_dispatch`)**，不得自带 `on: push/pull_request/tags`。
- **门控矩阵**：
  | 事件 | tests | core | studio/mobile/docker | go/js/nix/ohos | release |
  |---|---|---|---|---|---|
  | `feature` push | ✓ | ✓（仅 linux-x86_64，无 magisk） | ✗ | ✗ | ✗ |
  | PR(develop/main) | ✓ | ✓（全量） | ✗ | ✗ | ✗ |
  | `develop`/`main` push | ✓ | ✓（全量+magisk） | ✓（必跑） | 按路径门控 | ✗ |
  | tag `v*` | ✓ | ✓（全量） | ✓ | **全量**（绕门控） | ✓ |
  | `workflow_dispatch` | ✓ | ✓（全量） | ✓ | **全量**（绕门控） | ✗ |
- **不因内容重复跳过构建**：`core/test/mobile` 的 `should_skip` 仅由 PR 路径门控决定（`fkirc/skip-duplicate-actions` 的输出不再影响构建；其 `cancel_others` 仍用于取消被取代的运行）。原因：下游 `docker`/`release` 依赖本轮 `core` 的产物，跳过会导致取不到。
- **发版产物经 Actions artifact 传递**（`core-*`/`studio-*`/`mobile-*`/`magisk` 前缀上传；`release.yml` 用 `download-artifact` 的 `pattern` 精确拉取）。不用 `actions/cache`（10GB 配额 LRU 驱逐不可靠），也不用全量 `download-artifact`（buildx 产生的 `<owner>~<repo>~*.dockerbuild` 名字含 `~`，会导致解压失败）。
- **发布**：`release.yml` 自动发布（`draft:false`），`version` = tag 名；Windows/macOS 资产由 `productName`（`tauri.conf.json`）决定命名与 MSI **UpgradeCode** —— **切勿轻改 `productName`**，否则升级会变成并存新装。
- **仓库可见性**：默认 **private**。私仓 Actions 额度耗尽时，需要跑 CI/发版可**临时** `gh api -X PATCH repos/exvlan/exvlan -f visibility=public`，跑完**立即改回 private**。
- **发布 go/sdk**（仅 tag，且**失败阻塞 release**）：`publish-npm.yml` 将 `@exvlan/runtime|browser|cloudflare|sdk` 发到 **GitHub Packages npm**（`npm.pkg.github.com`，token=`GITHUB_TOKEN`，dist-tag=`latest`，手动可传）；`publish-go.yml` 推 Go module tag **`exvlan/go/<tag>`**（消费需 `GOPRIVATE=github.com/exvlan/*`）。`release` 作业的 `needs` 含这两个作业。
- **同提交去重**：`pipeline.yml` 的 `concurrency.group` 用 `github.sha`（非 `github.ref`）；发版时**先 push develop、再 push tag** → 同 sha 下 tag 运行胜出（含 release），分支运行被取消，只跑一轮。

## 版本对齐范围

- 标准版本 `YY.QMM.Mmpp` 覆盖：根 `Cargo.toml`/各 crate、`packages/{studio,std,ui,generator}/package.json`、`tauri.conf.json`、magisk 清单、README 等。
- **`packages/sdk/**` 一并纳入**（sdk 为嵌套独立 workspace）：需同时改各子包 `version` 与内部 `workspace:<v>` specifier，并重生成 `packages/sdk/pnpm-lock.yaml`；否则 specifier 不匹配、`ERR_PNPM_OUTDATED_LOCKFILE`。

<!--VITE PLUS START-->

# Using Vite+, the Unified Toolchain for the Web

This project is using Vite+, a unified toolchain built on top of Vite, Rolldown, Vitest, tsdown, Oxlint, Oxfmt, and Vite Task. Vite+ wraps runtime management, package management, and frontend tooling in a single global CLI called `vp`. Vite+ is distinct from Vite, and it invokes Vite through `vp dev` and `vp build`. Run `vp help` to print a list of commands and `vp <command> --help` for information about a specific command.

Docs are local at `node_modules/vite-plus/docs` or online at https://viteplus.dev/guide/.

## Built-in Commands vs Scripts

`vp <name>` runs a built-in command. `vp run <name>` runs a `package.json` script or a `vite.config.ts` task. Scripts cannot overwrite built-ins, so `vp dev` and `vp run dev` may do different things. Check `package.json` and `vite.config.ts` first, and run `vp run <name>` when the project defines a script or task with that name.

## Review Checklist

- [ ] Run `vp install` after pulling remote changes and before getting started.
- [ ] Run `vp check` and `vp test` to format, lint, type check and test changes.
- [ ] Check if there are `vite.config.ts` tasks or `package.json` scripts necessary for validation, run via `vp run <script>`.
- [ ] If setup, runtime, or package-manager behavior looks wrong, run `vp env doctor` and include its output when asking for help.

<!--VITE PLUS END-->

## Docker 操作边界（硬约束）

- 只允许 stop/rm/run **明确命名的本项目测试容器**（`ws-test` 等 `ws-*` 前缀）
- 禁止 stop/rm 任何其他容器（mysql-test/ingress/caddy/dufs/pgsql/redis/valkey/page-spy/windows/dockur/backups/webdav 等基础设施/其他项目）
- 禁止全量 docker 操作：`docker rm -f $(docker ps -aq)`、`docker stop $(docker ps -q)`、`docker system prune -af`
- 禁止 `docker compose down`（depend/ 基础设施——除非用户明确授权）
- 操作前 `docker ps` 核对容器归属；不确定则不操作
