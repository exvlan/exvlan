# EXVlan 项目约束

## 分支合并策略

- **上游只进 feature**：`develop => feature`（develop 同步进 feature）；`upstream/releases/v.* => feature`（上游 EasyTier releases 只合入 feature）。
- **feature => (squash) develop**：feature 以 **squash** 方式并入 develop。
- **develop 保持独立**：develop 不允许混入任何上游链（上游 = EasyTier 等外部仓库），其历史应始终为：本地基点（如 `382534d0` / `9779923b`）→ 单个 squash 提交。
- 禁止对 develop 直接 merge 上游分支。

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
