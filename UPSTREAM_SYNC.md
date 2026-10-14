# 上游同步速查卡（Upstream Sync）

> 权威全文见 [`docs/rules/workflow/upstream-sync.md`](./docs/rules/workflow/upstream-sync.md)。
> 本文仅作一页速查，口径以全文为准。

## 分支模型

```
upstream  ──releases/v*──►  feature  ──squash──►  develop
                                        ▲                      │
                                        └──── merge ───────────┘
                                        (develop 同步进 feature)

develop = 线性独立历史（0 merge，无上游链）
feature = 整合中枢（上游 release + develop + 本地特性）
```

## 三步命令

```bash
# A. 上游 release → feature（merge，非 rebase）
git fetch upstream --tags --prune
git checkout feature && git merge --no-ff upstream/releases/vX.Y.Z

# B. develop → feature（内容一致时用 -s ours 建立祖先关系）
git fetch origin && git merge origin/develop
# 需要时：git merge -s ours origin/develop --no-edit

# C. feature →（squash）develop
git checkout develop && git merge --squash feature
git commit -m "feat: <批次描述>（压缩 <a>..<b>）"
```

## 护栏

- **禁止** 对 `develop` 直接 `merge upstream/*`；**禁止** rebase `develop` 到上游。
- **禁止** `develop` 出现 merge 提交（只能线性 / squash）；`feature` 才允许 merge。
- 冲突取 **feature 已验证版本**（CI/config）；仅在 develop 侧为明确行为修正时回退 develop。

## 校验

```bash
git rev-list --merges --count develop                         # 期望 0
git merge-base --is-ancestor upstream/main develop            # 期望 1（非祖先）
git merge-base --is-ancestor upstream/releases/vX.Y.Z feature # 期望 0（是祖先）
```

## 版本对齐

- 标准：`YY.QMM.Mmpp`，如 `26.410.20700` = 2026 年 Q4 10 月，对齐上游 `v2.7.0`。
- Windows MSI：`YY.MMQ.mmmm-pp`（仅 `tauri.windows.conf.json` 生效）。
- **`packages/sdk/**` 纳入**：需同改子包 `version` + `workspace:<v>` specifier + 重生成 sdk 锁文件。

## CI 编排（单入口）

- 唯一入口 `pipeline.yml`；其余工作流仅 `workflow_call`(+`workflow_dispatch`)，不得自带 push/PR/tag 触发。
- 门控：`feature`=test+core(x86_64)；PR=test+core；`develop`/`main`=全链+旁路路径门控；tag/dispatch=全量+release。
- `core/test/mobile` 不因内容重复跳过（否则 `docker`/`release` 取不到产物）；发版产物走 Actions artifact
  （`core-*`/`studio-*`/`mobile-*`/`magisk`；`release.yml` 用 `download-artifact` 的 `pattern` 拉取）。
- 发布自动（`draft:false`）；`productName` 决定 Windows MSI UpgradeCode，勿轻改。
- 私仓 Actions 额度耗尽时临时 public 跑 CI，完毕立即改回 private。
- **发布 go/sdk**（仅 tag，失败阻塞 release）：npm→GitHub Packages（`@exvlan/runtime|browser|cloudflare|sdk`）；Go→module tag `exvlan/go/<tag>`。
- **同提交去重**：pipeline `concurrency` 按 `github.sha`；发版先 push develop 再 push tag，只跑一轮。
