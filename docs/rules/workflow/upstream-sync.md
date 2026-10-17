---
时间: 2026-10-10 18:55
版本: v26.410.20702
引用源: [AGENTS.md](../../../AGENTS.md) / [CLAUDE.md](../../../CLAUDE.md) / git 历史实测
状态: 已完成
---

# 历史上游同步工作流程（Upstream Sync Workflow）

> 本文为**权威全文**（canonical）。根目录 [`UPSTREAM_SYNC.md`](../../../UPSTREAM_SYNC.md) 为速查卡；
> `AGENTS.md` / `CLAUDE.md` 的《分支合并策略》为面向 AI 的约束摘要。三处口径以本文为准。

## 1. 目的与适用范围

- 记录 EXVlan 从外部上游同步代码的历史做法与现行规范。
- 约束对象：`develop`、`feature` 两条长期分支，以及四个远程 `upstream` / `origin` / `cloud` / `std-v`。
- 适用动作：上游 release 合并、develop 与 feature 互同步、feature squash 进 develop、版本对齐、冲突处理。
- 不适用：本地特性分支的日常开发（按 `CONTRIBUTING.md` 从 develop 切 feature/* 即可）。

## 2. 远程与分支模型

### 2.1 远程

| 远程       | 仓库                                   | 角色                                      |
| ---------- | -------------------------------------- | ----------------------------------------- |
| `upstream` | `git@github.com:EasyTier/EasyTier.git` | 外部上游（只读来源，禁止向 develop 直连） |
| `origin`   | `git@github.com:exvlan/exvlan.git`     | 主公库                                    |
| `cloud`    | `git@github.com:era-cloud/exvlan.git`  | 发布/签名 fork                            |
| `std-v`    | `git@github.com:std-v/exvlan.git`      | 发布/签名 fork                            |

### 2.2 分支

| 分支      | 定位                    | 历史形态                                                                             |
| --------- | ----------------------- | ------------------------------------------------------------------------------------ |
| `develop` | **稳定主干 / 独立历史** | 线性，**0 个 merge 提交**；基点 `9779923b`(2024) → `382534d0`(2026) → 各 squash 批次 |
| `feature` | **整合中枢**            | 含 merge 提交；承载上游 release、develop 同步与本地特性                              |

**核心不变量**：`develop` 不得混入任何上游链（如上游仓库等外部来源）。判定 `upstream/*` 是否为 `develop` 祖先必须为假。

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

| 提交                    | 动作                         | 上游来源                                             |
| ----------------------- | ---------------------------- | ---------------------------------------------------- |
| `28357200`              | merge → `dev/feature`        | `upstream/releases/v2.5.0` (2026-01-01)              |
| `6222a8d1`              | merge → `dev/feature`        | `upstream/releases/v2.6.0` (2026-04-06)              |
| `e977a302`              | merge → `feature`            | `upstream/main`                                      |
| `2d17564b`              | merge → `feature`            | `upstream/releases/v2.6.1` (2026-04-19)              |
| `1e284ff4`              | merge → `feature`            | `upstream/releases`（带入 v2.6.2 / v2.6.3 / v2.6.4） |
| `e4c50e72`              | `origin/develop` → `feature` | 冲突取 feature 已验证版本                            |
| `31c4eeae`              | `origin/develop` → `feature` | 常规同步                                             |
| `d634f7a6`              | `develop` → `feature`        | 内容一致，**仅建立祖先关系**（bridge）               |
| `cc2430ea` / `8c5c2fc7` | `develop` → `feature`        | 常规同步                                             |

**结论**：`upstream/releases/v2.6.0`~`v2.6.4` 均为 `feature` 祖先；`upstream/main` **不是** `feature`/`develop` 的祖先——即只取**打了标签的 release**，不追 `main`。

## 5. 标准操作流程（命令级）

### A. 上游 release → feature

```bash
# 一次性：配置上游远程
git remote add upstream git@github.com:EasyTier/EasyTier.git

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
  - 示例：`26.410.20702` = 2026 年 Q4 10 月，对齐上游 `v2.7.0`。
- Windows MSI 专用版本：`YY.MMQ.mmmm-pp`（因 MSI 中段每段上限 255/255/65535），仅经
  `packages/studio/src-tauri/tauri.windows.conf.json` 对 Windows 生效。
- 版本号需同步更新各 `Cargo.toml` / `package.json` / `magisk` 清单 / README 版本串；
  详见 `c21eac5d version: v26.308.20604`。
- **`packages/sdk/**` 一并纳入**（嵌套独立 workspace）：需同时改各子包 `version` 与内部 `workspace:<v>`
  specifier，并重生成 `packages/sdk/pnpm-lock.yaml`；否则 specifier 不匹配、`ERR_PNPM_OUTDATED_LOCKFILE`。

### E. CI 编排（统一 pipeline 单入口）

- **唯一入口** `pipeline.yml`；其余工作流（`test/core/studio/mobile/docker/release/go/js/nix/ohos`）
  只保留 `workflow_call`(+`workflow_dispatch`)，不得自带 `on: push/pull_request/tags`。
- **门控**：`feature`=test+core(仅 linux-x86_64、无 magisk)；PR(develop/main)=test+core；
  `develop`/`main`=全链(core/studio/mobile/docker 必跑)+旁路(go/js/nix/ohos)按路径门控；
  tag `v*` 与 `workflow_dispatch`=全量（绕路径门控）+ `release`(仅 tag)。
- **不因内容重复跳过** `core/test/mobile`（`should_skip` 仅由 PR 路径门控决定；`fkirc/skip-duplicate-actions`
  输出不再影响构建，其 `cancel_others` 保留），否则下游 `docker`/`release` 取不到本轮 core 产物。
- **发版产物经 Actions artifact 传递**（`core-*`/`studio-*`/`mobile-*`/`magisk` 上传；
  `release.yml` 用 `download-artifact` 的 `pattern` 精确拉取）。不用 `actions/cache`（配额 LRU 驱逐），
  也不用全量 download（buildx 的 `*~*.dockerbuild` 名含 `~` 会解压失败）。
- **发布**：`release.yml` 自动 `draft:false` 发布，`version`=tag 名；`productName`（`tauri.conf.json`）
  决定 Windows MSI **UpgradeCode** 与资产命名，**勿轻改**（改了升级会变并存新装）。
- **可见性**：默认 private；私仓 Actions 额度耗尽时，跑 CI/发版可临时
  `gh api -X PATCH repos/exvlan/exvlan -f visibility=public`，完毕立即改回 private。
- **发布 go/sdk**（仅 tag，**失败阻塞 release**）：`publish-npm.yml` 发
  `@exvlan/runtime|browser|cloudflare|sdk` 到 GitHub Packages npm；`publish-go.yml` 推
  Go module tag `exvlan/go/<tag>`（消费需 `GOPRIVATE=github.com/exvlan/*`）。`release` 的 `needs` 含二者。
- **同提交去重**：pipeline `concurrency.group` 按 `github.sha`；发版“先 push develop 再 push tag”，
  同 sha 下 tag 运行胜出、分支运行被取消（只一轮）。

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
git merge-base --is-ancestor upstream/releases/v2.6.4 develop # 期望: 退出码 1（非祖先）

# 3) 已同步的 release 必须是 feature 祖先
git merge-base --is-ancestor upstream/releases/v2.6.4 feature # 期望: 退出码 0（是祖先）
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
