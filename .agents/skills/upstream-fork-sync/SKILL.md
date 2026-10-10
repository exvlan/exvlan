---
name: upstream-fork-sync
description: 同步上游 release（EasyTier）到重命名/重构过的 fork（EXVlan）。覆盖路径映射、merkle 合并策略、冲突分层裁决、不变量校验、构建验证与收口。触发词：上游同步、sync upstream vX.Y.Z、fork rebrand merge、合并 easytier 上游。
---

# upstream-fork-sync

把外部上游（EasyTier）的某个 release 同步进本 fork（EXVlan），同时保持 fork 的品牌、目录布局与本地行为修正。

## 何时使用

- 需要把 `upstream/releases/vX.Y.Z` 合并进 `feature`。
- 需要校验/更新上游路径 ↔ 本仓库布局的映射。
- 需要检查分支不变量（develop 线性、上游非祖先、release 已并入 feature）。

## 单一事实源

- 路径映射：`docs/rules/workflow/upstream-path-map.yaml`
- 流程与历史：`docs/rules/workflow/upstream-sync.md`
- 分支约束：`AGENTS.md` / `CLAUDE.md` 的《分支合并策略》

## 分支模型（不变量）

- `develop`：线性独立历史，**0 个 merge 提交**，不含任何上游链。
- `feature`：整合中枢，承载上游 release + develop 同步 + 本地特性。
- 上游只进 `feature`；`develop` 只允许线性/ squash 推进。

## 标准流程

1. **读 manifest**：`docs/rules/workflow/upstream-path-map.yaml`。
2. **冻结**：`git tag pre-sync-<ver> feature`；`git branch backup/feature-pre-<ver> feature`。
3. **取源**：`git fetch upstream --tags --prune`；`git rev-parse <tag>` 校验。
4. **baseline**（可选但推荐）：在 feature 记录既有测试失败，便于合并后对比。
5. **建同步分支**：`git switch -c sync/<ver> feature`。
6. **docs 提交**（merge 前）：更新映射/文档/skill。
7. **单个 merge 提交**：`git merge --no-ff upstream/releases/<ver>`，在冲突消解中完成
   布局搬迁 / 删除 / 重命名（`plan_renames.sh` 生成计划）。
8. **依赖/版本对齐**。
9. **验证**：`check_invariants.sh` + 构建矩阵（见下）。
10. **收口**：`git switch feature && git merge --ff-only sync/<ver> && git push origin feature`。

## 冲突分层裁决

| 冲突面                               | 处置                                      |
| ------------------------------------ | ----------------------------------------- |
| 品牌 / 命名 / CI / 安装器 / 版本串   | 以 fork 为准                              |
| Rust 核心行为 / 协议 / 依赖版本      | 以上游 release 为准                       |
| 本地明确行为修正（如端口 `:0` 语义） | 保留本地                                  |
| CI / 配置基线                        | 取 feature 已验证版本，仅吸收上游必要新增 |
| 锁文件                               | 不手工合并，构建时重新生成                |

## 验证矩阵

```bash
cargo fmt && cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run            # 若无 nextest 可用 cargo test 兜底
cargo hack check
vp check && vp test
wasm-pack build exvlan/core --target wasm32-wasip1   # 如涉及 wasi
( cd exvlan/go && go build ./... && go test ./... )  # 如涉及 go
python3 .agents/skills/upstream-fork-sync/scripts/map_paths.py --tag <ver>   # 无遗漏
bash .agents/skills/upstream-fork-sync/scripts/check_invariants.sh
```

## 工具脚本

- `scripts/map_paths.py --tag <upstream-tag>`：读取 manifest，列出上游路径 → 目标映射，并标记未映射项（`❓`）。
- `scripts/check_invariants.sh`：校验分支不变量。
- `scripts/plan_renames.sh --tag <upstream-tag>`：依据 manifest 生成 merge 期的 `git mv`/搬迁计划。

## 红线

- 禁止对 `develop` 直接 `merge upstream/*`。
- 禁止 rebase `develop` 到上游。
- 禁止在 `develop` 上做 merge 提交。
- 映射变更先改 manifest，再同步文档与脚本。
