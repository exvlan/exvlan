# 全仓安全审查报告（后门 / 遥测 / 数据收集）

- 日期：2026-10-15
- 范围：`exvlan/**`、`packages/**`、`.github/workflows/**`、`script/**`、构建脚本、依赖清单、二进制/wasm 制品、安装脚本、`install.sh/ps1`、magisk 模块
- 方法：静态模式扫描 + 外联面枚举（源码 + 二进制/wasm `strings`）+ 依赖审计（`cargo audit`/`pnpm audit`/`govulncheck`）+ CI 数据流审查 + 运行中服务**非侵入**动态抽流
- 工具：`grep/strings/jq/ss`；`cargo-audit 0.22.2`、`govulncheck`、`pnpm audit`（官方 registry）
- 原始证据：`/tmp/opencode/audit-2026-10-15/`

## 0. 结论摘要

**未发现后门、隐蔽遥测或未授权的数据收集。** 主要风险集中在：**依赖漏洞**、**CI 供应链（action 未钉 SHA）**、**同名配置注入面（`sh -c`）**、以及**一处向第三方个人仓库的发布外发**。

| 严重度 | 数量 | 条目                                                                                |
| ------ | ---- | ----------------------------------------------------------------------------------- |
| High   | 4    | H1 配置注入到 `sh -c`；H2 CI action 未钉 SHA；H3 pnpm 依赖漏洞；H4 cargo 依赖漏洞   |
| Medium | 2    | M1 Go stdlib 漏洞；M2 ohos 向第三方仓库 dispatch                                    |
| Low    | 3    | L1 `ALIYUN_OSS_BUCKET` 冗余 secret；L2 CI 安装器 `curl\|sh`；L3 未固定分支 git 依赖 |
| Info   | 若干 | 特权脚本 base64 配置；遥测缺席；动态抽流结论；二进制无隐藏 host；误报说明；信任边界 |

---

## 1. High

### H1. 配置可控字符串拼入 `sh -c`（macOS/FreeBSD 特权守护进程）—— 命令注入面

- 证据：`exvlan/exvlan/src/common/ifcfg/mod.rs:145`
  ```rust
  cmd_out = Command::new("sh").arg("-c").arg(cmd).output().await?;   // macOS/FreeBSD 分支
  ```
  调用方以 `format!` 拼接**配置可控**字段，例如 `exvlan/exvlan/src/common/ifcfg/darwin.rs:65`
  ```rust
  run_shell_cmd(format!("ifconfig {} {}", name, if up {"up"} else {"down"}))
  ```
  其中 `name` 即接口名，来源为配置 `flags.dev_name`（`exvlan/core/src/config/toml.rs`）。
- 影响：若配置（尤其经 `-w` config server 下发）含 shell 元字符，可致**特权命令注入**（守护进程以 root 运行）。
- 说明：Linux 路径走 netlink（`ip`/`netlink.rs`，argv 直传）**不受此影响**；风险主要在 **macOS/FreeBSD**。
- 建议：避免 `sh -c`，改为 `Command::new("ifconfig").args([...])` 的 argv 直传；或对 `dev_name`/地址做严格白名单校验（`^[A-Za-z0-9_.-]{1,15}$`）。

### H2. CI 供应链：`uses:` 全部按 major tag（非 commit SHA）

- 证据：`.github/workflows/*.yml` 中所有 `uses:` 均为 `@vN`（`actions/checkout@v5`、`pnpm/action-setup@v5`、`ErBWs/setup-ohos@v1`、`signpath/...@v2` 等）。
- 影响：上游（含第三方个人 action）可移动 tag → 在被引用后注入代码。
- 建议（仅建议，本次未改）：关键 action 钉 **commit SHA**；对第三方（非 GitHub 官方）action 尤其重要。

### H3. 前端依赖漏洞（pnpm audit：18 枚，11 high / 7 moderate）

- `axios`（多枚，high/moderate）：`>=1.13.0 <1.20.0` 的 SSRF/prototype-pollution/ReDoS/DoS/header-injection 等；出现在 **`packages/std`、`packages/ui`**。
- `js-yaml`（high/moderate）：`>=4.0.0 <4.3.2` ReDoS/原型污染。
- `@vue/server-renderer`（high）：`<3.5.42` XSS。
- 建议：升级 `axios>=1.20.0`、`js-yaml>=4.3.2`、`@vue/server-renderer>=3.5.42`，并重生成相应锁文件。

### H4. Rust 依赖漏洞（cargo audit：7 枚）

- `rustls 0.23.45`：`RUSTSEC-2026-0346`（`Debug` 泄露会话密钥）、`-0347`（ECH 拒后向外部服务器发送缓冲数据）、`-0348`（Brotli 证书解压**认证前最多分配 1GiB**）、`-0349`（ServerHello 降级到经典密钥交换）。
- `hickory-proto 0.25.2`：`RUSTSEC-2026-0118`（NSEC3 校验无界循环）、`-0119`（O(n²) 名称压缩 CPU 耗尽）。
- `rsa 0.9.10`：`RUSTSEC-2023-0071`（Marvin 定时侧信道）。
- 另有 5 条 `unmaintained/unsound` 警告（`glib`/`ttf-parser`/`paste`/`proc-macro-error`/`encoding`）。
- 建议：升级 `rustls`、`hickory-proto`；`rsa` 多为传递依赖，评估可达性与替换。

---

## 2. Medium

### M1. Go 标准库漏洞（govulncheck：5 枚**可达**）

- 证据：`go1.24.4` 的 `net/url`（`GO-2025-4010` 等）、`os/exec`（`GO-2025-3956`），命中 `internal/host/instance_config.go`、`cmd/update-wasm/main.go`。
- 建议：提升 Go toolchain（`go.mod`/`toolchain`）到已修复版本；重跑 `govulncheck` 归零可达项。

### M2. 发布外发：ohos 向第三方（个人）仓库 dispatch

- 证据：`.github/workflows/ohos.yml`（Publish 步骤）用 `DOWNSTREAM_DISPATCH_TOKEN` `POST` 到
  `FrankHan052176/Exvlan-ArkTS`、`FrankHan052176/exvlan-pro-app`，载荷含 `repository/ref/package`。
- 影响：属**向第三方个人账户仓库外发**（非本组织）。虽非“数据窃取”，但属于**供应链/数据面外部依赖**。
- 判定：**需文档化或调整**。建议：改指向本组织（`exvlan/*`）或明确 approval；移除/替换个人仓库目标。

---

## 3. Low / Info

- **L1 `ALIYUN_OSS_BUCKET` 冗余 secret**：`core.yml:126`、`mobile.yml:74` 注入 `OSS_BUCKET: ${{ secrets.ALIYUN_OSS_BUCKET }}`，但**全仓无任何引用**（Rust/JS/其他 workflow 均未用）→ 无外发行为，属**多余的 secret 暴露**。建议移除该 env。
- **L2 CI 安装器管道**：`Dockerfile.mobile` 的 `curl … | sh`（rustup/volta）、`exvlan/contrib/android-jni/README.md:33`（文档）→ 建议钉版本 + 校验哈希。
- **L3 未固定分支 git 依赖**：`exvlan/exvlan/Cargo.toml:204` `service-manager-rs { branch = "main" }`（其余 `EasyTier/*` 已钉 rev/版本；`kcp-sys`、`windivert-rust` 已钉 rev）。建议钉 rev。
- **Info 特权脚本 base64 配置**：`exvlan/contrib/magisk/webui.sh`（`save-and-restart`）解码 base64 → 写 `config.toml` → **先 `exvlan --check-config` 校验**再替换、`umask 077`、临时文件原子替换。设计较稳；需确保 WebUI 调用方鉴权。
- **Info 遥测缺席**：全仓未见 `sentry/posthog/mixpanel/GA/umami/...` 等遥测 SDK 被打入产物；`@opentelemetry/api` 仅作为某工具链的 **optional peer** 出现在 `pnpm-lock.yaml`（未启用）。
- **Info 动态抽流（生产服务，非侵入）**：`ss` 显示其出站**仅** mesh 对端（IPv6 `…:11010`、`172.30.0.2:11011`、自身公网 `8.219.215.202`）与 `loopback`；监听仅 `15888`(RPC) 与本地隧道。**无陌生外联/无遥测域名**。
- **Info 二进制/wasm**：`strings`（4 个 wasm）未见隐藏 host/URL，仅含 crate 源码路径、加密库与错误串（`bytecodec/stun_codec/aes-gcm/getrandom` 等）。
- **Info 误报说明**：硬编码 `secret/token` 字符串均为**测试夹具**（`secret123`、`credential-secret`、`not-a-private-key`）；`s3.ax1x.com`=验证码文档图床、`dpgr.am`=Deepgram 文档链接、`opencollective.com`=捐赠链接。
- **Info 信任边界**：`-w` config server 可向客户端**下发实例配置**（含 `dev_name` 等）；与 H1 组合时风险放大。建议将 config server 视为高信任源，并在客户端对下发字段做校验/白名单。

---

## 4. 明确“未发现”的类别

- 未发现**后门**：无隐藏回调、无“下载后可执行”模式、无加密 blob 执行、无异常 OS 进程外呼；进程执行均为网络接口配置/提权/测试。
- 未发现**遥测/分析**：无第三方分析 SDK 打包，无 phone-home。
- 未发现**隐蔽数据收集**：动态抽流仅见 mesh 正常流量；未见向外发送配置/密钥/标识到第三方。

## 5. 建议优先级

1. 升级依赖：`axios>=1.20.0`、`js-yaml>=4.3.2`、`@vue/server-renderer>=3.5.42`（H3）；`rustls`/`hickory-proto`（H4）；Go toolchain（M1）。
2. 修复 H1：`ifcfg` 去 `sh -c` 改 argv 直传 / `dev_name` 白名单校验。
3. 供应链：CI 关键 action 钉 SHA、`service-manager-rs` 钉 rev（H2/L3）。
4. 发布外发：调整/文档化 ohos 对第三方仓库的 dispatch（M2）。
5. 清理：移除 `ALIYUN_OSS_BUCKET` env（L1）；CI 安装器钉版本（L2）。

> 说明：本报告为**静态 + 非侵入动态**审查，未对运行中服务注入故障；依赖审计基于当日 advisory 数据库。供应链修复按约定**仅建议、未直接实施**。

---

## 6. 处置结果（本次实施）

| 发现                                        | 处置                                                                                                                                                                                                                                |
| ------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| H1 配置注入到 `sh -c`                       | **已修**：`ifcfg` 改为 `Command::new(prog).args([...])` **argv 直传**，并新增 `validate_ifname`（`dev_name` 白名单 `^[A-Za-z0-9_.-]{1,15}$`）。                                                                                     |
| H2 CI action 未钉 SHA                       | **已加固**：`.github/workflows/*` 中全部 `uses:` 钉到 **commit SHA**（含 `# <tag>` 注释）；`actions/setup-java@v6`、`android-actions/setup-android@v4` 顺带升级以处理 Node20 告警（`arduino/setup-protoc` 无更高 major，保留 v3）。 |
| H3 前端依赖漏洞（18）                       | **已修**：`axios ^1.20.0`、`vue ^3.5.42`（含 `@vue/server-renderer`）、`js-yaml` 定向 override `^4.3.2`；`pnpm audit` → **0 漏洞**。                                                                                                |
| H4 Rust 依赖漏洞（7）                       | **部分修复**：`rustls 0.23.45 → 0.23.46`（清除 4 枚）。剩余 `hickory-proto 0.25.2`（2 枚，**上游未发补丁**，见 hickory-dns PR #3597/#3613）、`rsa 0.9.10`（Marvin，传递依赖）→ **待上游**。                                         |
| M1 Go stdlib（5）                           | CI 使用 `go-version: 1.25.x`（≥ 修复版 1.25.8）；本地 `govulncheck` 的 5 枚源自旧的 go1.24.4，CI 侧不应复现（保持监控）。                                                                                                           |
| M2 第三方仓库 dispatch                      | **文档化保留**：`ohos.yml` 增加显式注释说明目标为外部个人仓库、需授权。                                                                                                                                                             |
| L1 冗余 `OSS_BUCKET`                        | **已移除**：`core.yml`、`mobile.yml` 中的 `OSS_BUCKET` env 删除。                                                                                                                                                                   |
| L2 CI 安装器 `curl\|sh` / L3 未固定分支依赖 | 待办（未改）。                                                                                                                                                                                                                      |
