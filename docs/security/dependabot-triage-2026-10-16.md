# Dependabot 告警分类（2026-10-16）

> 来源：`gh api --paginate repos/exvlan/exvlan/dependabot/alerts?state=open`（默认分支 `develop`）。
> 与 `docs/security/repo-audit-2026-10-15.md` 的差异：Dependabot 覆盖面更广（全部 manifest/lockfile、含 dev 依赖），且**不读未跟踪的根 lockfile**。

## 1. 总览（116 项）

| 维度 | 分布 |
| --- | --- |
| 严重度 | critical 9、high 56、medium 47、low 4 |
| 生态 | npm 107、rust 8、go 1 |
| manifest | `packages/std/pnpm-lock.yaml` 71、`packages/sdk/pnpm-lock.yaml` 20、`packages/std/package.json` 8、`Cargo.lock` 5、`packages/sdk/examples/web/package.json` 3、`exvlan/exvlan/Cargo.toml` 3、`packages/studio/package.json` 2、`packages/sdk/runtime/package.json` 1、`packages/sdk/cloudflare/package.json` 1、`packages/sdk/browser/package.json` 1、`exvlan/go/examples/tun/go.mod` 1 |

## 2. 关键结构性发现（影响 71/116 项的判读）

- `packages/std` 是**根 pnpm workspace 的成员**（见 `pnpm-workspace.yaml`），根 `pnpm-lock.yaml` 才是权威锁；
- 但仓库**跟踪了陈旧的 `packages/std/pnpm-lock.yaml`**（解析为 axios 1.18.0 / vitest 2.1.9 / happy-dom 16.8.1 —— 远旧于实际），而**根 `pnpm-lock.yaml` 被 `.gitignore` 排除、未跟踪**；
- 结果：Dependabot 只能扫到那份陈旧嵌套锁 → **71 项告警来自它**，其中多数（axios/vue/js-yaml 等）**在本轮 26.410.20702 已通过根 lock 修复**，属判读噪声；
- 同时「根 lock 未跟踪」本身是问题：根 workspace 的安装不可复现、Dependabot 看不到真实依赖。

## 3. 可修项（有补丁版本）

| manifest | 包 | 生态 | 需 ≥ | 严重度 | 告警数 |
| --- | --- | --- | --- | --- | --- |
| `Cargo.lock` | glib | rust | 0.20.0 | medium | 1 |
| `packages/sdk/pnpm-lock.yaml` | esbuild | npm | 0.25.0 | medium | 1 |
| `packages/std/pnpm-lock.yaml` :x: 陈旧锁 | esbuild | npm | 0.25.0 | medium | 1 |
| `Cargo.lock` | hickory-proto | rust | 0.26.1 | medium | 1 |
| `exvlan/exvlan/Cargo.toml` | hickory-proto | rust | 0.26.1 | medium | 1 |
| `Cargo.lock` | hickory-resolver | rust | 0.26.2 | high/medium | 2 |
| `exvlan/exvlan/Cargo.toml` | hickory-resolver | rust | 0.26.2 | high/medium | 2 |
| `packages/sdk/pnpm-lock.yaml` | sharp | npm | 0.35.5 | high | 1 |
| `exvlan/go/examples/tun/go.mod` | golang.org/x/net | go | 0.55.0 | medium | 1 |
| `packages/sdk/pnpm-lock.yaml` | source-map-js | npm | 1.2.2 | high | 1 |
| `packages/std/pnpm-lock.yaml` :x: 陈旧锁 | source-map-js | npm | 1.2.2 | high | 1 |
| `packages/std/pnpm-lock.yaml` :x: 陈旧锁 | axios | npm | 1.20.0 | high/medium | 12 |
| `packages/sdk/pnpm-lock.yaml` | tinypool | npm | 2.1.2 | critical | 2 |
| `packages/std/pnpm-lock.yaml` :x: 陈旧锁 | tinypool | npm | 2.1.2 | critical | 2 |
| `packages/std/pnpm-lock.yaml` :x: 陈旧锁 | baseline-browser-mapping | npm | 2.11.0 | medium | 1 |
| `packages/std/package.json` | happy-dom | npm | 20.8.9 | critical/high | 3 |
| `packages/std/pnpm-lock.yaml` :x: 陈旧锁 | happy-dom | npm | 20.8.9 | critical/high | 3 |
| `packages/std/pnpm-lock.yaml` :x: 陈旧锁 | fast-uri | npm | 3.1.8 | high/medium | 8 |
| `packages/std/pnpm-lock.yaml` :x: 陈旧锁 | nanoid | npm | 3.3.18 | high/medium | 4 |
| `packages/std/pnpm-lock.yaml` :x: 陈旧锁 | @vue/server-renderer | npm | 3.5.42 | high | 1 |
| `packages/sdk/browser/package.json` | vitest | npm | 4.1.11 | medium | 1 |
| `packages/sdk/cloudflare/package.json` | vitest | npm | 4.1.11 | medium | 1 |
| `packages/sdk/pnpm-lock.yaml` | @vitest/mocker | npm | 4.1.11 | medium | 1 |
| `packages/sdk/pnpm-lock.yaml` | vitest | npm | 4.1.11 | medium | 1 |
| `packages/sdk/runtime/package.json` | vitest | npm | 4.1.11 | medium | 1 |
| `packages/std/package.json` | vitest | npm | 4.1.11 | critical/medium | 2 |
| `packages/std/pnpm-lock.yaml` :x: 陈旧锁 | @vitest/mocker | npm | 4.1.11 | medium | 1 |
| `packages/std/pnpm-lock.yaml` :x: 陈旧锁 | vitest | npm | 4.1.11 | critical/medium | 2 |
| `packages/studio/package.json` | vitest | npm | 4.1.11 | critical/medium | 2 |
| `packages/std/pnpm-lock.yaml` :x: 陈旧锁 | browserslist | npm | 4.28.7 | high | 2 |
| `packages/std/pnpm-lock.yaml` :x: 陈旧锁 | js-yaml | npm | 4.3.2 | high/medium | 5 |
| `packages/std/pnpm-lock.yaml` :x: 陈旧锁 | rollup | npm | 4.59.0 | high | 1 |
| `packages/std/pnpm-lock.yaml` :x: 陈旧锁 | brace-expansion | npm | 5.0.12 | high/medium/low | 14 |
| `packages/sdk/examples/web/package.json` | vite | npm | 6.4.3 | high/medium | 3 |
| `packages/sdk/pnpm-lock.yaml` | vite | npm | 6.4.3 | high/medium | 3 |
| `packages/std/package.json` | vite | npm | 6.4.3 | high/medium | 3 |
| `packages/std/pnpm-lock.yaml` :x: 陈旧锁 | vite | npm | 6.4.3 | high/medium | 3 |
| `packages/std/pnpm-lock.yaml` :x: 陈旧锁 | postcss-selector-parser | npm | 7.1.6 | medium | 1 |
| `packages/sdk/pnpm-lock.yaml` | undici | npm | 7.29.1 | high/medium/low | 10 |
| `packages/std/pnpm-lock.yaml` :x: 陈旧锁 | postcss | npm | 8.5.23 | high/medium | 4 |
| `packages/std/pnpm-lock.yaml` :x: 陈旧锁 | minimatch | npm | 9.0.7 | high | 3 |

## 4. 无补丁项（需监控或接受）

| 严重度 | 生态 | 包 | GHSA | manifest |
| --- | --- | --- | --- | --- |
| high | npm | braces | GHSA-vfj7-8cjw-p6xm | `packages/std/pnpm-lock.yaml` |
| high | rust | hickory-proto | GHSA-3v94-mw7p-v465 | `Cargo.lock` |
| medium | npm | sprintf-js | GHSA-hp3w-g68c-fv3c | `packages/std/pnpm-lock.yaml` |

## 5. 处置与跟踪

| 范围 | 对应 Issue |
| --- | --- |
| rust：hickory-proto≥0.26.1 / hickory-resolver≥0.26.2 / glib≥0.20 | [#1](https://github.com/exvlan/exvlan/issues/1) |
| go：golang.org/x/net≥0.55.0 | [#2](https://github.com/exvlan/exvlan/issues/2) |
| npm：`packages/sdk` 升级 | [#3](https://github.com/exvlan/exvlan/issues/3) |
| npm：`packages/std` + `packages/studio` package.json 范围 | [#4](https://github.com/exvlan/exvlan/issues/4) |
| hygiene：陈旧嵌套锁 + 根锁未跟踪 | [#5](https://github.com/exvlan/exvlan/issues/5) |
| 无补丁项监控（braces / hickory-proto GHSA-3v94 / sprintf-js） | [#6](https://github.com/exvlan/exvlan/issues/6) |

补充说明：

- **修复优先级**：[#5](https://github.com/exvlan/exvlan/issues/5) 先做（它决定其余 71 项是否仍需处理）；其余按 #1（rust，影响运行时）> #3/#4（构建/测试链）> #2 > #6。
- **回归防护**：CI 已增设依赖审计门禁（`cargo audit` + `pnpm audit --prod`，含 allow-list），新增漏洞会直接红灯。

## 6. 执行进度（2026-10-17）

| Issue | 状态 | 结果 |
| --- | --- | --- |
| [#1](https://github.com/exvlan/exvlan/issues/1) rust | **未完成（上游阻塞）** | `hickory-client/server` 无稳定 0.26（仅 `0.26.0-alpha.1`），四件套需协同；`glib 0.18.5` 属 **gtk-rs 0.18 栈**（gtk/webkit2gtk/libappindicator），升 0.20 需 tauri/wry 大版本升级。已在 `.cargo/audit.toml` 登记理由。 |
| [#2](https://github.com/exvlan/exvlan/issues/2) go | ✅ 已修 | `golang.org/x/net v0.50.0 → v0.55.0`（x/sys 0.45）；`go build/test ./...` 全绿。 |
| [#3](https://github.com/exvlan/exvlan/issues/3) sdk | ✅ 已修 | vitest **4.1.12**、vite **6.4.4**；传递依赖 override：sharp **0.35.5** / source-map-js **1.2.2** / undici **7.30.1**；sdk test 全绿。 |
| [#4](https://github.com/exvlan/exvlan/issues/4) std+studio | ✅ 已修 | vitest **4.1.12**、happy-dom **20.14.6**；std 的 `vite` 规格改为 `catalog:`（root override 本就强制 vite-plus-core）。测试 38 + 15 全绿。 |
| [#5](https://github.com/exvlan/exvlan/issues/5) 结构 | ✅ 已修 | 删除陈旧 `packages/std/pnpm-lock.yaml`（71 项误报根因）；根锁按决策维持未跟踪。 |
| [#6](https://github.com/exvlan/exvlan/issues/6) 监控 | 持续 | 三项无补丁告警已入 allow-list / 分类文档。 |

其他：已开启 **Dependabot security updates**（仅为安全告警自动开 PR，不开常规 version updates）；审计门禁保持 `--prod` 范围（决策）。

### 复核（2026-10-17，修复后）

Dependabot 开启告警：**116 → 8**（-93%）。

| 生态 | 剩余 | 明细 |
| --- | --- | --- |
| rust | 8 | `hickory-proto` ×3、`hickory-resolver` ×4、`glib` ×1（即 [#1](https://github.com/exvlan/exvlan/issues/1) 的上游阻塞项） |
| npm | **0** | 107 项全部关闭（升级 + 删除陈旧嵌套锁） |
| go | **0** | 已修 |

严重度：high 3 / medium 5。剩余项均已由 `.cargo/audit-allowlist.toml`（含到期复核）与 issue #1 跟踪。




