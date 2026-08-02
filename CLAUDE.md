# EXVlan 项目约束

## 分支合并策略

- **上游只进 feature**：`develop => feature`（develop 同步进 feature）；`upstream/releases/v.* => feature`（上游 EasyTier releases 只合入 feature）。
- **feature => (squash) develop**：feature 以 **squash** 方式并入 develop。
- **develop 保持独立**：develop 不允许混入任何上游链（上游 = EasyTier 等外部仓库），其历史应始终为：本地基点（如 `382534d0` / `9779923b`）→ 单个 squash 提交。
- 禁止对 develop 直接 merge 上游分支。
