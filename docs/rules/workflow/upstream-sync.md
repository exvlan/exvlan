---
时间: 2026-10-10 19:30
版本: v2.7.0
引用源: [AGENTS.md](../../../AGENTS.md) / [CLAUDE.md](../../../CLAUDE.md) / [upstream-path-map.yaml](./upstream-path-map.yaml) / git 历史实测
状态: 进行中（v2.7.0 同步）
---

# 历史上游同步工作流程（Upstream Sync Workflow）

> 本文为**权威全文**（canonical）。根目录 [`UPSTREAM_SYNC.md`](../../../UPSTREAM_SYNC.md) 为速查卡；
> `AGENTS.md` / `CLAUDE.md` 的《分支合并策略》为面向 AI 的约束摘要。三处口径以本文为准。

## 1. 目的与适用范围

- 记录 Exvlan 从外部上游 **exvlan/exvlan** 同步代码的历史做法与现行规范。
- 约束对象：`develop`、`feature` 两条长期分支，以及四个远程 `upstream` / `origin` / `cloud` / `std-v`。
- 适用动作：上游 release 合并、develop 与 feature 互同步、feature squash 进 develop、版本对齐、冲突处理。
- 不适用：本地特性分支的日常开发（按 `CONTRIBUTING.md` 从 develop 切 feature/* 即可）。

## 2. 远程与分支模型

### 2.1 远程

| 远程       | 仓库                                   | 角色                                      |
| ---------- | -------------------------------------- | ----------------------------------------- |
| `upstream` | `git@github.com:exvlan/exvlan.git` | 外部上游（只读来源，禁止向 develop 直连） |
| `origin`   | `git@github.com:exvlan/exvlan.git`     | 主公库                                    |
| `cloud`    | `git@github.com:era-cloud/exvlan.git`  | 发布/签名 fork                            |
| `std-v`    | `git@github.com:std-v/exvlan.git`      | 发布/签名 fork                            |

### 2.2 分支

| 分支      | 定位                    | 历史形态                                                                             |
| --------- | ----------------------- | ------------------------------------------------------------------------------------ |
| `develop` | **稳定主干 / 独立历史** | 线性，**0 个 merge 提交**；基点 `9779923b`(2024) → `382534d0`(2026) → 各 squash 批次 |
| `feature` | **整合中枢**            | 含 merge 提交；承载上游 release、develop 同步与本地特性                              |

**核心不变量**：`develop` 不得混入任何上游链（Exvlan 等外部仓库）。判定 `upstream/*` 是否为 `develop` 祖先必须为假。

### 2.3 路径映射与可复用工具

- **映射单一事实源**：[`upstream-path-map.yaml`](./upstream-path-map.yaml)（上游路径 → 我方布局）。
- **可复用 skill**：[`.agents/skills/upstream-fork-sync/`](../../../.agents/skills/upstream-fork-sync/SKILL.md)
  - `scripts/map_paths.py`：按 manifest 映射上游路径并标记未映射项（无遗漏校验）。
  - `scripts/check_invariants.sh`：本节 §7 三条不变量校验。
  - `scripts/plan_renames.sh`：生成 merge 期的 `git mv` / 搬迁计划。
- **约定**：映射变更先改 manifest，再同步本文与 skill，避免多处漂移。

## 3. 历史复原（两阶段）

### 阶段一：`dev/feature` + `dev/squash` 双分支模型（已废弃）

早期同时存在 `dev/feature` 与 `dev/squash` 两条分支，二者互相 merge，且 `dev/feature` **直接并入上游 release**：

- `28357200` `Merge remote-tracking branch 'upstream/releases/v2.5.0' into dev/feature`
- `6222a8d1` `Merge remote-tracking branch 'upstream/releases/v2.6.0' into dev/feature`
- 期间 `dev/squash` 参与压缩（提交信息可见 `Merge branch 'dev/squash' into dev/feature` 等）

该阶段的旧引用被保留在 `refs/original/` 下（说明此后对历史做过重写/`filter-branch`）：

```
refs/original/refs/heads/dev/feature
refs/original/refs/heads/develop
refs/original/refs/heads/feature/ci
refs/original/refs/remotes/ci-test/develop
refs/original/refs/remotes/ci-test/feature/ci
```

### 阶段二：现行模型（`develop` 独立 + 上游只进 `feature`）

`b2c96192`（2026-08-02，`docs: 记录分支合并策略（上游只进 feature，feature squash 进 develop）`）
将该策略固化进 `AGENTS.md` / `CLAUDE.md`，并随后把 `develop` 重写为线性独立历史。

## 4. 同步事件时间线（git 实测）

| 提交                    | 动作                         | 上游来源                                                |
| ----------------------- | ---------------------------- | ------------------------------------------------------- |
| `28357200`              | merge → `dev/feature`        | `upstream/releases/v2.5.0` (2026-01-01)                 |
| `6222a8d1`              | merge → `dev/feature`        | `upstream/releases/v2.6.0` (2026-04-06)                 |
| `e977a302`              | merge → `feature`            | `upstream/main`                                         |
| `2d17564b`              | merge → `feature`            | `upstream/releases/v2.6.1` (2026-04-19)                 |
| `1e284ff4`              | merge → `feature`            | `upstream/releases`（带入 v2.6.2 / v2.6.3 / v2.6.4）    |
| `e4c50e72`              | `origin/develop` → `feature` | 冲突取 feature 已验证版本                               |
| `31c4eeae`              | `origin/develop` → `feature` | 常规同步                                                |
| `d634f7a6`              | `develop` → `feature`        | 内容一致，**仅建立祖先关系**（bridge）                  |
| `cc2430ea` / `8c5c2fc7` | `develop` → `feature`        | 常规同步                                                |
| `（本次）sync/v2.7.0`   | merge → `feature`            | `upstream/releases/v2.7.0` (2026-10-05) — 单 merge 提交 |

**结论**：`upstream/releases/v2.6.0`~`v2.7.0` 均为 `feature` 祖先；`upstream/main` **不是** `feature`/`develop` 的祖先——即只取**打了标签的 release**，不追 `main`。

> **v2.7.0 特别说明**：上游 v2.7.0 为**大规模 workspace 重构**（新增 `exvlan-core`(wasi)/`exvlan-proto`/`exvlan-go`/`exvlan-js`，删除 `exvlan-rpc-build`，`exvlan-api` 拆为前后端多包）。本次同步采用
> 「**单个 merge 提交 + merge 消解内完成布局搬迁/删除/重命名**」：
> `git mv exvlan/core exvlan/exvlan`、`git rm exvlan/build`、`git rm exvlan/rpc`、`git mv exvlan-proto exvlan/proto`；
> 依赖链对齐为 `exvlan/exvlan → exvlan-core → exvlan-proto`。详见 [`upstream-path-map.yaml`](./upstream-path-map.yaml)。

## 5. 标准操作流程（命令级）

### A. 上游 release → feature

```bash
# 一次性：配置上游远程
git remote add upstream git@github.com:exvlan/exvlan.git

# 拉取上游标签与分支
git fetch upstream --tags --prune

# 以 merge（禁止 rebase）方式把某个 release 并入 feature
git checkout feature
git merge --no-ff upstream/releases/v2.6.4

# 冲突处理见 §6；随后推送
git push origin feature
```

> 历史采用 `merge remote-tracking branch 'upstream/releases/vX.Y.Z' into feature` 形式；
> 若一次带入多个 release，可直接 merge 上游的聚合分支（如 `upstream/releases`）。

### B. develop → feature（含建立祖先关系 bridge）

```bash
git fetch origin
git checkout feature

# 常规同步
git merge origin/develop

# 当 develop 与 feature 内容已一致、但需为后续 squash/PR 建立共同祖先时：
git merge -s ours origin/develop --no-edit   # 仅记录祖先关系，不改变 feature 内容
```

> `-s ours` 对应历史上的 `d634f7a6`「merge: develop 同步进 feature（内容一致，建立祖先关系）」。
> 目的：让 `feature` 成为 `develop` 的后代，保证后续 `feature => develop` 的 squash/PR 能正常收敛。

### C. feature →（squash）develop

```bash
git checkout develop
git merge --squash feature

# 单提交收口；保留「（压缩 <range>）」信息约定
git commit -m "feat: <批次描述>（压缩 <a>..<b>）"
git push origin develop
```

> 提交信息约定参见历史样例 `69ff75e1`：
> `fix: 版本串/双引擎修复 + primevue 退回 4.5.5（压缩 9bf19e77..5c89010e）`。
> 也可通过 GitHub 的 **Squash and merge** 完成，效果等价。

### D. 版本对齐

- 标准版本：`YY.QMM.Mmpp`，其中 `YY`=年、`QMM`=季度+月份（zero-padded）、`Mmpp`=major + minor(2位) + patch(2位)。
  - 示例：`26.308.20604` = 2026 年 Q3 8 月，对齐上游 `v2.6.4`。
- Windows MSI 专用版本：`YY.MMQ.mmmm-pp`（因 MSI 中段每段上限 255/255/65535），仅经
  `packages/studio/src-tauri/tauri.windows.conf.json` 对 Windows 生效。
- 版本号需同步更新各 `Cargo.toml` / `package.json` / `magisk` 清单 / README 版本串；
  详见 `c21eac5d version: v26.308.20604`。

## 6. 冲突解决策略

| 冲突面                                | 处置原则                               | 历史依据   |
| ------------------------------------- | -------------------------------------- | ---------- |
| CI / 配置文件（`.github/`、构建脚本） | **取 feature 侧已验证版本**            | `e4c50e72` |
| 行为/语义差异（如端口 `:0` 处理）     | 回退 develop 侧改动，保留 feature 语义 | `853ec77b` |
| 合并后格式                            | `cargo fmt` 收敛，保持 diff 干净       | `29dba3f5` |

原则：**冲突以 feature（已验证的上游整合结果）为准，除非 develop 侧为明确的本地行为修正。**

## 7. 不变量与护栏（校验命令）

```bash
# 1) develop 必须保持线性（无 merge 提交）
git rev-list --merges --count develop        # 期望: 0

# 2) develop 不得含上游链
git merge-base --is-ancestor upstream/main develop            # 期望: 退出码 1（非祖先）
git merge-base --is-ancestor upstream/releases/v2.7.0 develop # 期望: 退出码 1（非祖先）

# 3) 已同步的 release 必须是 feature 祖先
git merge-base --is-ancestor upstream/releases/v2.7.0 feature # 期望: 退出码 0（是祖先）
```

护栏红线：

- **禁止** 对 `develop` 直接 `merge upstream/*`。
- **禁止** 把 `develop` rebase 到任何上游链之上。
- **禁止** 在 `develop` 上做 merge 提交（只能线性提交 / squash）。
- `feature` 允许 merge；`develop` 只允许线性推进。

## 8. 同步前 / 后检查清单

**同步前**

- [ ] `git fetch upstream --tags --prune && git fetch origin` 已更新引用
- [ ] 确认目标上游 release 标签（`git tag -l 'v2.*'`）与待对齐版本
- [ ] 工作区干净（`git status`）

**同步后**

- [ ] `cargo fmt` / `cargo clippy`（如有 Rust 改动）
- [ ] `vp check && vp test`（如有前端改动）
- [ ] 执行 §7 三条校验全部符合期望
- [ ] 版本串已按 §5-D 对齐
- [ ] `git push origin feature` / `git push origin develop` 完成

## 9. 附录：关键提交索引

| 类别                | 提交                                                   |
| ------------------- | ------------------------------------------------------ |
| 策略固化            | `b2c96192` / `CLAUDE.md` `AGENTS.md`                   |
| 上游合并            | `28357200` `6222a8d1` `e977a302` `2d17564b` `1e284ff4` |
| develop 同步 bridge | `d634f7a6`                                             |
| 冲突策略样例        | `e4c50e72` `853ec77b`                                  |
| 版本对齐            | `c21eac5d`                                             |
| squash 提交信息约定 | `69ff75e1`                                             |
| develop 基点        | `9779923b` `382534d0`                                  |
