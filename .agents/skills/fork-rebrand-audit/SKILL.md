---
name: fork-rebrand-audit
description: Fork 去品牌（rebrand）审计与遗留回归修复。用于重命名/重构/去上游品牌后的全仓清理、白名单保留、二进制夹具处理，以及 rebrand 引入的编译/运行回归排查。触发词：去品牌、rebrand、去上游品牌、debrand、全仓替换品牌名、rebrand 回归、清除 XXX 字样。
---

# fork-rebrand-audit

把 fork 从上游品牌名改为自有品牌（或反之），并**系统性排查 rebrand 引入的遗留回归**。
原则：**文本层去品牌要彻底，但必须白名单保留技术坐标；二进制与生成物不可盲目文本替换。**

## 何时使用

- 需要把 fork 里的上游品牌名从代码/文档/CI 中清除。
- 上游合并（见 `upstream-fork-sync`）后出现「编译通过但运行/初始化报错」「锁文件不一致」「路径找不到」等疑似 rebrand 回归。
- 需要审计全仓残留品牌字样与应当保留的坐标。

## 去品牌范围与白名单

| 类别 | 处理 |
| --- | --- |
| 代码/文档/CI/注释里的**品牌名** | 改为自有品牌（或泛称“外部上游”） |
| **外部依赖坐标** | **保留原名**：`github.com/<upstream>/*`（`Cargo.toml`/`Cargo.lock`）、`tun-<upstream>` 等包名 |
| **许可证文本** | **保留**（如 `LICENSE`，法务必需） |
| **上游路径映射清单** | **保留**（同步工具依赖，如 `docs/rules/workflow/upstream-path-map.yaml`） |
| **git remote URL** | 视为技术坐标，可保留（或泛化，但会损失可操作信息） |
| **二进制制品 / 测试夹具** | 见下节 |

> 判定：删了会**破坏构建/同步/法务**的，属白名单；纯品牌叙述可去。

## 常量分类（防误伤）

| 类别 | 例子 | 处理 |
| --- | --- | --- |
| **A. 品牌/本地名** | 日志文件、socket、CLI、`user_agent`、tracing target | 自有品牌 |
| **B. 宿主 ABI 名** | WASI host 模块/导出（guest wasm ↔ native/JS/Go 宿主） | fork 内部，**多面必须同向**；不上网 |
| **C. 网络 wire 常量** | KDF 域分隔、Noise prologue、proof 串、HKDF label | fork **自洽**即可 → 自有品牌（不要求与上游互通；需同步改断言测试） |
| **外部依赖** | `tun-<upstream>`、`github.com/<upstream>/*` | **保留原名** |

**原则**：不要求与上游节点互通，但**功能/协议机制与上游一致**；ABI 改动后必须**重建 wasm**。

## 二进制 / 生成物处理（关键经验）

- **生成代码**（`*.pb.go`、proto 生成物）：**不可**文本替换品牌名——会破坏内嵌 `rawDesc`，导致运行时 `panic: slice bounds out of range`。应**用生成器重生成**（确保 generator 版本与运行时一致）。
- **预编译 wasm 夹具**：可用 `wasm-tools print <f.wasm> -o f.wat` → 用 `sed` 改**导入模块名/导出名** → `wasm-tools parse f.wat -o f.wasm`（无损往返；`wasm-tools validate` 校验）。
  - ⚠️ **只改导入/导出/符号（name 段）是安全的**；**改数据段内的字符串（如内嵌服务名、type-URL）会改变数据长度、破坏按固定偏移读取的代码** → 数据段内的品牌串**无法在不重编源码的前提下安全改名**，应保留（或重建源码）。
  - 可 `wasm-tools strip --all` 去除 `name/producers/target_features` 等 custom 段（去掉大量符号串，减小体积），不影响运行。
- **锁文件/specifier**：嵌套 workspace 的子包 `version` 若改动，必须同步 `workspace:<v>` specifier 并重生成锁文件（否则 `ERR_PNPM_OUTDATED_LOCKFILE`）。
- **脚本相对路径**：relayout 后脚本里 `../..`-类路径可能偏移一层（如 repo-root 计算错误 → `Cannot find module` / `Failed opening .../target/...`）。

## rebrand 回归清单（合并/去品牌后逐项验证）

- [ ] `bash .agents/skills/fork-rebrand-audit/scripts/debrand_scan.sh`（残余 + 白名单核对）
- [ ] Go：`go build ./... && go test ./...`（重点 proto/生成物）
- [ ] JS/TS：嵌套 workspace `install --frozen-lockfile` + `check`（specifier/锁文件/工具链）
- [ ] Rust：`cargo fmt && cargo clippy -D warnings` + 目标构建矩阵
- [ ] wasm/ABI：重建并校验（宿主多面一致）
- [ ] 夹具/制品：`wasm-tools validate` + 相关一致性测试
- [ ] 白名单：确认依赖坐标 / LICENSE / 映射清单**仍在**

## 工具脚本

- `scripts/debrand_scan.sh [pattern]`：扫描全仓残余品牌字样，并列出**白名单**（依赖坐标/LICENSE/path-map）以核对哪些属“应保留”。
- `scripts/rebrand_regression_check.sh [--repo <dir>]`：rebrand 回归抽查——生成物是否含可疑文本替换、嵌套 workspace 锁文件一致性、脚本路径是否偏移。

## 红线

- 禁止对**生成代码/二进制数据段**做盲文本替换。
- 禁止删除**依赖坐标 / 许可证 / 上游路径映射**（构建、同步与法务依赖）。
- 去品牌后必须跑完整验证矩阵，不能只看编译通过。
