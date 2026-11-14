//! 统一的「更新」模块：**内核**与**应用**共用同一套 manifest / 校验 / 下载逻辑。
//!
//! 两个源（镜像端点）：
//! - 内核：`<base>/core/manifest.json`（`versions[].assets[platform] = { file, sha256, entry }`）
//! - 应用：`<base>/upgrade/manifest.json`（同结构，`entry` 可缺省）
//!
//! 平台键：`macos-x86_64` / `windows-x86_64` / `linux-x86_64` / `android-arm64-v8a` …
//!
//! 说明：`upgrade::kernel` 在移动端没有意义（内核随包），调用会返回明确错误；
//! `upgrade::app` 全平台可用（桌面走安装包，Android 走 APK）。

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

/// 应用更新清单目录（镜像）。
pub const APP_BASE_URL: &str = "https://dl.exvlan.era-x.cn/upgrade/";
/// 内核更新清单目录（镜像）。
pub const KERNEL_BASE_URL: &str = "https://dl.exvlan.era-x.cn/core/";

const MANIFEST_SCHEMA: u32 = 1;

// ---------------------------------------------------------------------------
// 共用层
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Manifest {
    #[serde(default)]
    pub schema: u32,
    #[serde(default)]
    pub latest: String,
    #[serde(default)]
    pub versions: Vec<VersionEntry>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VersionEntry {
    pub version: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub rpc_schema: u32,
    #[serde(default)]
    pub min_studio_version: String,
    #[serde(default)]
    pub assets: BTreeMap<String, Asset>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Asset {
    pub file: String,
    #[serde(default)]
    pub sha256: String,
    /// 压缩包内的入口文件名（仅内核使用）。
    #[serde(default)]
    pub entry: String,
}

/// 当前平台在 manifest 里的键名。
///
/// 桌面三端与服务端**同一真源**（[`exvlan_control::platform_key`]）：
/// `macos-x86_64` / `windows-x86_64` / `linux-aarch64` …（Windows 的 arm64 键
/// 是 `arm64`）。Android/iOS 走 ABI 键，只有客户端用得到，留在本地。
pub fn platform_key() -> String {
    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        let abi = if cfg!(target_arch = "aarch64") {
            "arm64-v8a"
        } else if cfg!(target_arch = "arm") {
            "armeabi-v7a"
        } else if cfg!(target_arch = "x86") {
            "x86"
        } else {
            "x86_64"
        };
        format!("android-{abi}")
    }

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        exvlan_control::platform_key()
    }
}

pub fn normalize_base(base: &str, default: &str) -> String {
    let trimmed = base.trim();
    let trimmed = if trimmed.is_empty() { default } else { trimmed };
    if trimmed.ends_with('/') {
        trimmed.to_string()
    } else {
        format!("{trimmed}/")
    }
}

/// 解析 `YY.QMM.Mmpp` 版本号用于比较。
pub fn version_key(v: &str) -> Vec<u64> {
    v.trim()
        .trim_start_matches('v')
        .split(['.', '-', '+'])
        .filter_map(|s| s.parse::<u64>().ok())
        .collect()
}

pub fn version_ge(a: &str, b: &str) -> bool {
    if b.trim().is_empty() {
        return true;
    }
    version_key(a) >= version_key(b)
}

async fn fetch_manifest(base: &str, default_base: &str) -> anyhow::Result<Manifest> {
    let base = normalize_base(base, default_base);
    let url = format!("{base}manifest.json");
    let resp = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()?
        .get(&url)
        .send()
        .await
        .with_context(|| format!("请求 {url} 失败"))?;
    if !resp.status().is_success() {
        bail!("{url} 返回 {}", resp.status());
    }
    let manifest: Manifest = resp
        .json()
        .await
        .with_context(|| format!("解析 {url} 失败"))?;
    if manifest.schema > MANIFEST_SCHEMA {
        bail!(
            "manifest schema 为 {}，当前客户端仅支持 {}",
            manifest.schema,
            MANIFEST_SCHEMA
        );
    }
    Ok(manifest)
}

// ---------------------------------------------------------------------------
// 更新通道（stable < rc < beta < dev < nightly）
// ---------------------------------------------------------------------------
//
// 两张**不同**的排序表，别混：
// - `visibility_rank`：选中某档能看哪些版本（向下包含：nightly 连 stable 也看）
// - `compare_stage`：同一个 base 下谁更新（stable > rc > beta > dev > nightly）

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    Stable,
    Rc,
    Beta,
    Dev,
    Nightly,
}

impl Channel {
    pub const ALL: [Channel; 5] = [
        Channel::Stable,
        Channel::Rc,
        Channel::Beta,
        Channel::Dev,
        Channel::Nightly,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Channel::Stable => "stable",
            Channel::Rc => "rc",
            Channel::Beta => "beta",
            Channel::Dev => "dev",
            Channel::Nightly => "nightly",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "stable" => Some(Channel::Stable),
            "rc" | "pre" => Some(Channel::Rc),
            "beta" => Some(Channel::Beta),
            "dev" => Some(Channel::Dev),
            "nightly" => Some(Channel::Nightly),
            _ => None,
        }
    }

    /// 该档位能看到的版本（含更低风险档位）。
    pub fn visibility_rank(self) -> u8 {
        match self {
            Channel::Stable => 0,
            Channel::Rc => 1,
            Channel::Beta => 2,
            Channel::Dev => 3,
            Channel::Nightly => 4,
        }
    }

    /// 同一个 base 下的先后：正式版是 rc 的后一步，所以 stable 最大。
    pub fn compare_stage(self) -> u8 {
        match self {
            Channel::Stable => 5,
            Channel::Rc => 4,
            Channel::Beta => 3,
            Channel::Dev => 2,
            Channel::Nightly => 1,
        }
    }

    /// 安装的是哪条线（首次启动没有偏好时用版本串推断）。
    /// 历史 `-pre.N` 归到 dev（pre 线就是开发线）。
    pub fn infer_from_version(version: &str) -> Self {
        let lower = version.to_ascii_lowercase();
        if lower.contains("-nightly.") {
            Channel::Nightly
        } else if lower.contains("-dev.") || lower.contains("-pre.") {
            Channel::Dev
        } else if lower.contains("-beta.") {
            Channel::Beta
        } else if lower.contains("-rc.") {
            Channel::Rc
        } else {
            Channel::Stable
        }
    }

    /// 需要请求的通道目录（从低风险到高风险，顺序稳定便于测试）。
    pub fn visible(self) -> Vec<Channel> {
        Channel::ALL
            .into_iter()
            .filter(|c| c.visibility_rank() <= self.visibility_rank())
            .collect()
    }
}

/// 结构化版本：`(base, stage, seq)`。
///
/// 改这个是因为旧的 `version_key` 把 `-pre.20` 当成第 4 段数字 —— 同 base 下
/// `26.410.20712`（正式版）会被判成比 `26.410.20712-pre.20` 更旧。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub base: Vec<u64>,
    pub stage: u8,
    /// legacy `-pre.*` 用 0（比 nightly 还低，只影响历史版本排序）
    pub seq: u64,
}

impl Version {
    pub fn parse(raw: &str) -> Self {
        let text = raw.trim().trim_start_matches('v');
        let text = text.split('+').next().unwrap_or(text);
        let (base_part, suffix) = match text.split_once('-') {
            Some((b, s)) => (b, Some(s)),
            None => (text, None),
        };
        let base: Vec<u64> = base_part
            .split('.')
            .filter_map(|p| p.parse::<u64>().ok())
            .collect();
        let (stage, seq) = match suffix {
            None => (Channel::Stable.compare_stage(), 0),
            Some(suffix) => {
                let (name, seq) = suffix
                    .split_once('.')
                    .map(|(n, v)| (n, v.parse::<u64>().unwrap_or(0)))
                    .unwrap_or((suffix, 0));
                let stage = match name.to_ascii_lowercase().as_str() {
                    "pre" => 0,
                    "nightly" => Channel::Nightly.compare_stage(),
                    "dev" => Channel::Dev.compare_stage(),
                    "beta" => Channel::Beta.compare_stage(),
                    "rc" => Channel::Rc.compare_stage(),
                    _ => 0,
                };
                (stage, seq)
            }
        };
        Self { base, stage, seq }
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.base
            .cmp(&other.base)
            .then(self.stage.cmp(&other.stage))
            .then(self.seq.cmp(&other.seq))
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// 通道目录下的清单地址：`<base><channel>/manifest.json`。
pub fn channel_manifest_url(base: &str, default_base: &str, channel: Channel) -> String {
    format!(
        "{}{}/manifest.json",
        normalize_base(base, default_base),
        channel.as_str()
    )
}

/// 通道目录下的官方 updater 清单：`<base><channel>/latest.json`。
pub fn channel_latest_url(base: &str, default_base: &str, channel: Channel) -> String {
    format!(
        "{}{}/latest.json",
        normalize_base(base, default_base),
        channel.as_str()
    )
}

/// 拉某个通道目录的清单。
pub async fn fetch_channel_manifest(
    base: &str,
    default_base: &str,
    channel: Channel,
) -> anyhow::Result<Manifest> {
    let url = channel_manifest_url(base, default_base, channel);
    let resp = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()?
        .get(&url)
        .send()
        .await
        .with_context(|| format!("请求 {url} 失败"))?;
    if !resp.status().is_success() {
        bail!("{url} 返回 {}", resp.status());
    }
    let manifest: Manifest = resp
        .json()
        .await
        .with_context(|| format!("解析 {url} 失败"))?;
    if manifest.schema > MANIFEST_SCHEMA {
        bail!(
            "manifest schema 为 {}，当前客户端仅支持 {}",
            manifest.schema,
            MANIFEST_SCHEMA
        );
    }
    Ok(manifest)
}

/// 合并「当前通道及以下」各通道清单里的版本。
///
/// 单个通道 404/网络错误只跳过（新通道刚上线时目录可能还不存在）；
/// 全部失败才算错误，避免断网时静默当作「无更新」。
pub async fn collect_channel_versions(
    base: &str,
    default_base: &str,
    channel: Channel,
) -> anyhow::Result<Vec<(Channel, VersionEntry)>> {
    let mut out: Vec<(Channel, VersionEntry)> = Vec::new();
    let mut last_err: Option<anyhow::Error> = None;
    for ch in channel.visible() {
        match fetch_channel_manifest(base, default_base, ch).await {
            Ok(manifest) => {
                for entry in manifest.versions {
                    out.push((ch, entry));
                }
            }
            Err(err) => last_err = Some(err),
        }
    }
    if out.is_empty()
        && let Some(err) = last_err
    {
        return Err(err.context("所有通道清单都拉不到"));
    }
    out.sort_by(|a, b| Version::parse(&a.1.version).cmp(&Version::parse(&b.1.version)));
    out.reverse();
    Ok(out)
}

/// 清单里的一个候选版本（给 UI 的「选择版本」下拉）。
#[derive(Debug, Clone, Serialize)]
pub struct VersionChoice {
    pub version: String,
    /// 它来自哪条通道（合并后才知道）
    pub channel: String,
    /// 当前客户端能不能装；false 时 `reason` 说明原因
    pub compatible: bool,
    pub reason: Option<String>,
    pub notes: Option<String>,
}

/// 当前客户端支持的 RPC 契约版本（内核 manifest 里的 `rpc_schema` 超过它就装不了）。
pub const SUPPORTED_RPC_SCHEMA: u32 = 1;

/// 在「当前通道及以下」里找指定版本，返回 (来源通道, 条目)。
pub async fn find_version(
    base: &str,
    default_base: &str,
    channel: Channel,
    version: &str,
) -> anyhow::Result<(Channel, VersionEntry)> {
    for ch in channel.visible() {
        if let Ok(manifest) = fetch_channel_manifest(base, default_base, ch).await
            && let Some(entry) = pick(&manifest, version)
        {
            return Ok((ch, entry.clone()));
        }
    }
    bail!("{version} 不在 {} 及以下任何通道的清单里", channel.as_str())
}

/// 下载并校验 sha256，返回字节。
async fn download_verified(base: &str, asset: &Asset) -> anyhow::Result<Vec<u8>> {
    let url = format!("{base}{}", asset.file);
    let bytes = reqwest::get(&url)
        .await
        .with_context(|| format!("下载 {url} 失败"))?
        .error_for_status()
        .with_context(|| format!("下载 {url} 返回错误状态"))?
        .bytes()
        .await
        .with_context(|| format!("读取 {url} 响应失败"))?;

    if !asset.sha256.is_empty() {
        let actual = format!("{:x}", Sha256::digest(&bytes));
        if !actual.eq_ignore_ascii_case(&asset.sha256) {
            bail!("校验失败：期望 {}，实际 {}（已丢弃）", asset.sha256, actual);
        }
    }
    Ok(bytes.to_vec())
}

fn pick<'a>(manifest: &'a Manifest, version: &str) -> Option<&'a VersionEntry> {
    manifest.versions.iter().find(|v| v.version == version)
}

// ---------------------------------------------------------------------------
// 内核更新
// ---------------------------------------------------------------------------

pub mod kernel {
    use super::*;

    /// 桌面端默认连的本地内核守护进程端口（仅用于提示）。
    pub const LOCAL_RPC_HINT: &str = "tcp://127.0.0.1:1995";

    #[derive(Debug, Clone, Serialize)]
    pub struct UpdateInfo {
        pub platform: String,
        pub current: Option<String>,
        pub latest: Option<String>,
        pub available: bool,
        pub versions: Vec<String>,
        pub source: String,
        /// 当前通道（决定看到哪些版本）
        pub channel: String,
        /// 各候选版本 + 兼容性（UI 里置灰不兼容项并给原因）
        pub choices: Vec<VersionChoice>,
    }

    /// 检查内核更新（移动端不支持：内核随包分发）。
    ///
    /// 合并「当前通道及以下」的清单，按 `(base, stage, seq)` 取最大的**兼容**版本。
    pub async fn check_update(
        base: &str,
        studio_version: &str,
        channel: Channel,
    ) -> anyhow::Result<UpdateInfo> {
        #[cfg(any(target_os = "android", target_os = "ios"))]
        {
            let _ = (base, studio_version, channel);
            bail!("移动端内核随应用发布，不支持在线升级");
        }

        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        {
            let base = normalize_base(base, KERNEL_BASE_URL);
            let platform = platform_key();
            let current = crate::kernel::installed_version();
            let merged = collect_channel_versions(&base, KERNEL_BASE_URL, channel).await?;

            let mut choices: Vec<VersionChoice> = Vec::new();
            let mut seen: Vec<String> = Vec::new();
            for (ch, entry) in merged {
                if seen.contains(&entry.version) {
                    continue;
                }
                if !entry.assets.contains_key(&platform) {
                    continue;
                }
                let reason = if entry.rpc_schema > SUPPORTED_RPC_SCHEMA {
                    Some(format!(
                        "该内核的 RPC 契约版本为 {}，当前客户端只支持 {}（请先升级客户端）",
                        entry.rpc_schema, SUPPORTED_RPC_SCHEMA
                    ))
                } else if !version_ge(studio_version, &entry.min_studio_version) {
                    Some(format!(
                        "需要客户端 ≥ {}（当前 {}）",
                        entry.min_studio_version, studio_version
                    ))
                } else {
                    None
                };
                seen.push(entry.version.clone());
                choices.push(VersionChoice {
                    version: entry.version,
                    channel: ch.as_str().to_string(),
                    compatible: reason.is_none(),
                    reason,
                    notes: (!entry.notes.is_empty()).then_some(entry.notes),
                });
            }

            let latest = choices
                .iter()
                .find(|c| c.compatible)
                .map(|c| c.version.clone());
            let available = match (&current, &latest) {
                (None, Some(_)) => true,
                (Some(c), Some(l)) => Version::parse(l) > Version::parse(c),
                _ => false,
            };
            let versions: Vec<String> = choices
                .iter()
                .filter(|c| c.compatible)
                .map(|c| c.version.clone())
                .collect();

            Ok(UpdateInfo {
                platform,
                current,
                latest,
                available,
                versions,
                source: base,
                channel: channel.as_str().to_string(),
                choices,
            })
        }
    }

    /// 下载并安装指定内核版本（需一次授权）。
    pub async fn install_version(
        base: &str,
        version: &str,
        studio_version: &str,
        channel: Channel,
    ) -> anyhow::Result<()> {
        #[cfg(any(target_os = "android", target_os = "ios"))]
        {
            let _ = (base, version, studio_version, channel);
            bail!("移动端内核随应用发布，不支持在线升级");
        }

        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        {
            let base = normalize_base(base, KERNEL_BASE_URL);
            let platform = platform_key();

            let (source_channel, entry) =
                find_version(&base, KERNEL_BASE_URL, channel, version).await?;
            if entry.rpc_schema > SUPPORTED_RPC_SCHEMA {
                bail!(
                    "内核 {version} 的 RPC 契约版本为 {}，当前 GUI 只支持 {}（请先升级客户端）",
                    entry.rpc_schema,
                    SUPPORTED_RPC_SCHEMA
                );
            }
            if !version_ge(studio_version, &entry.min_studio_version) {
                bail!(
                    "内核 {version} 要求 GUI ≥ {}（当前 {studio_version}）",
                    entry.min_studio_version
                );
            }
            let asset = entry
                .assets
                .get(&platform)
                .with_context(|| format!("内核 {version} 未提供 {platform} 平台的包"))?
                .clone();

            // 资产在通道目录下：`<base><channel>/<file>`
            let asset_base = format!("{}{}/", base, source_channel.as_str());
            let bytes = download_verified(&asset_base, &asset).await?;

            let tmp = tempfile::tempdir().context("创建临时目录失败")?;
            let archive = tmp.path().join("kernel.zip");
            std::fs::write(&archive, &bytes).context("写入临时文件失败")?;

            let entry_name = if asset.entry.is_empty() {
                crate::kernel::kernel_bin_name().to_string()
            } else {
                asset.entry.clone()
            };
            let binary = tmp.path().join(&entry_name);
            extract_entry(&archive, &entry_name, &binary)?;
            // 内核对运行时依赖（Windows: Packet.dll / wintun.dll / WinDivert*.sys）
            // 做同目录加载；在线更新时把 zip 里的动态库一并解出来，
            // 后面 install_kernel 会从 kernel_src 的父目录把它们复制到 core/。
            extract_runtime_deps(&archive, tmp.path())?;

            crate::run_elevated(&["--install-kernel", &binary.display().to_string(), version])
                .map_err(|e| anyhow::anyhow!(e))?;

            let _ = crate::run_elevated(&["--service-ctl", "stop"]);
            if let Err(err) = crate::run_elevated(&["--service-ctl", "start"]) {
                eprintln!("[kernel] 重启服务失败（内核已安装，可手动重启）: {err}");
            }

            Ok(())
        }
    }

    /// 从 zip 中解出入口二进制。
    /// 把内核包里的动态库（`*.dll` / `*.sys`）解到 `out_dir`（摊平目录，只取文件名）。
    ///
    /// 内核在 Windows 上从自己所在目录加载 `Packet.dll` / `wintun.dll` / `WinDivert*.sys`，
    /// 在线更新时必须一并带出来，否则更新后的内核会因为缺少依赖而跑不起来。
    fn extract_runtime_deps(archive: &Path, out_dir: &Path) -> anyhow::Result<()> {
        let file = std::fs::File::open(archive).context("打开内核包失败")?;
        let mut zip = zip::ZipArchive::new(file).context("解析内核包失败")?;

        for i in 0..zip.len() {
            let mut zf = zip.by_index(i)?;
            if zf.is_dir() {
                continue;
            }
            let name = zf.name()?.to_string();
            let lower = name.to_ascii_lowercase();
            if !(lower.ends_with(".dll") || lower.ends_with(".sys")) {
                continue;
            }
            // 只取文件名，避免 zip 里的路径穿越
            let Some(file_name) = Path::new(&name).file_name() else {
                continue;
            };
            let out = out_dir.join(file_name);
            let mut buf = Vec::with_capacity(zf.size() as usize);
            std::io::Read::read_to_end(&mut zf, &mut buf)?;
            std::fs::write(&out, &buf)
                .with_context(|| format!("写出运行时依赖 {} 失败", out.display()))?;
            crate::logger::record("kernel.deps", format!("extracted {}", out.display()));
        }
        Ok(())
    }

    fn extract_entry(archive: &Path, entry: &str, out: &Path) -> anyhow::Result<()> {
        let file = std::fs::File::open(archive).context("打开内核包失败")?;
        let mut zip = zip::ZipArchive::new(file).context("解析内核包失败")?;

        let mut index = None;
        for i in 0..zip.len() {
            let name = zip.by_index(i)?.name()?.to_string();
            if name == entry || name.ends_with(&format!("/{entry}")) {
                index = Some(i);
                break;
            }
        }
        let index = index.with_context(|| format!("内核包中找不到 {entry}"))?;

        let mut zf = zip.by_index(index)?;
        let mut buf = Vec::with_capacity(zf.size() as usize);
        std::io::Read::read_to_end(&mut zf, &mut buf)?;
        std::fs::write(out, &buf).context("写出内核二进制失败")?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(out, std::fs::Permissions::from_mode(0o755))?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// 应用更新
// ---------------------------------------------------------------------------

pub mod app {
    use super::*;

    #[derive(Debug, Clone, Serialize)]
    pub struct AppUpdateInfo {
        pub platform: String,
        pub current: String,
        pub latest: Option<String>,
        pub notes: Option<String>,
        pub available: bool,
        pub versions: Vec<String>,
        pub source: String,
        pub channel: String,
        /// 各候选版本（UI 的「选择版本」下拉用）
        pub choices: Vec<VersionChoice>,
    }

    /// 当前安装的应用版本。
    ///
    /// 必须用 `BUILD_VERSION`（CI 注入的 `<base>[-channel.N]+sha`）而不是
    /// `CARGO_PKG_VERSION`：nightly/dev 构建的 base 与 tag 不一致，用 crate 版本
    /// 会让客户端永远认为「有更新」（装完还提示同一个版本）。
    fn current_version() -> String {
        crate::BUILD_VERSION.to_string()
    }

    /// 检查应用更新（合并「当前通道及以下」的清单）。
    pub async fn check_update(base: &str, channel: Channel) -> anyhow::Result<AppUpdateInfo> {
        let base = normalize_base(base, APP_BASE_URL);
        let platform = platform_key();
        let current = current_version();
        let merged = collect_channel_versions(&base, APP_BASE_URL, channel).await?;

        let mut choices: Vec<VersionChoice> = Vec::new();
        for (ch, entry) in merged {
            if !entry.assets.contains_key(&platform) {
                continue;
            }
            if choices.iter().any(|c| c.version == entry.version) {
                continue;
            }
            choices.push(VersionChoice {
                version: entry.version,
                channel: ch.as_str().to_string(),
                compatible: true,
                reason: None,
                notes: (!entry.notes.is_empty()).then_some(entry.notes),
            });
        }

        let latest = choices.first().map(|c| c.version.clone());
        let available = latest
            .as_ref()
            .map(|l| Version::parse(l) > Version::parse(&current))
            .unwrap_or(false);
        let notes = choices.first().and_then(|c| c.notes.clone());
        let versions = choices.iter().map(|c| c.version.clone()).collect();

        Ok(AppUpdateInfo {
            platform,
            current,
            latest,
            notes,
            available,
            versions,
            source: base,
            channel: channel.as_str().to_string(),
            choices,
        })
    }

    /// 下载安装包（已校验 sha256），返回路径。
    ///
    /// `dest_dir` 为空时落到系统临时目录；Android 必须传应用 cache 目录，
    /// 否则 FileProvider 无法共享给系统安装器。
    pub async fn download_installer(
        base: &str,
        version: &str,
        channel: Channel,
        dest_dir: Option<&std::path::Path>,
    ) -> anyhow::Result<PathBuf> {
        let base = normalize_base(base, APP_BASE_URL);
        let platform = platform_key();
        let (source_channel, entry) = find_version(&base, APP_BASE_URL, channel, version).await?;
        let asset = entry
            .assets
            .get(&platform)
            .with_context(|| format!("应用 {version} 未提供 {platform} 平台的安装包"))?
            .clone();

        // 资产在通道目录下：`<base><channel>/<file>`
        let asset_base = format!("{}{}/", base, source_channel.as_str());
        let bytes = download_verified(&asset_base, &asset).await?;
        let dir = match dest_dir {
            Some(dir) => dir.to_path_buf(),
            None => std::env::temp_dir().join("exvlan-update"),
        };
        std::fs::create_dir_all(&dir).context("创建更新临时目录失败")?;
        let path = dir.join(&asset.file);
        std::fs::write(&path, &bytes).context("写入安装包失败")?;
        Ok(path)
    }

    /// 官方 updater 检查（桌面）：成功返回 Some，未配置/网络失败返回 None（由上层回退自建）。
    ///
    /// 端点按**通道**给：先算出「当前通道及以下」里最新的那一档，再把 updater 的
    /// endpoint 指向它 —— [`tauri_plugin_updater`] 自己只认单条线，无法做向下包含。
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    pub async fn check_update_official(
        app: &tauri::AppHandle,
        base: &str,
        channel: Channel,
    ) -> Option<AppUpdateInfo> {
        use tauri_plugin_updater::UpdaterExt;

        let base = normalize_base(base, APP_BASE_URL);
        let winner = match collect_channel_versions(&base, APP_BASE_URL, channel).await {
            Ok(list) => list
                .into_iter()
                .find(|(_, e)| e.assets.contains_key(&platform_key()))
                .map(|(ch, _)| ch),
            Err(err) => {
                eprintln!("[upgrade] 通道清单不可用: {err:#}");
                None
            }
        }?;

        let endpoint = channel_latest_url(&base, APP_BASE_URL, winner);
        let url = match tauri::Url::parse(&endpoint) {
            Ok(u) => u,
            Err(err) => {
                eprintln!("[upgrade] 非法 updater 端点 {endpoint}: {err}");
                return None;
            }
        };
        let updater = match app.updater_builder().endpoints(vec![url]) {
            Ok(builder) => match builder.build() {
                Ok(u) => u,
                Err(err) => {
                    eprintln!("[upgrade] 官方 updater 不可用: {err}");
                    return None;
                }
            },
            Err(err) => {
                eprintln!("[upgrade] 官方 updater 端点失败: {err}");
                return None;
            }
        };
        let found = match updater.check().await {
            Ok(found) => found,
            Err(err) => {
                eprintln!("[upgrade] 官方 updater 检查失败: {err}");
                return None;
            }
        };
        let current = current_version();
        let (latest, notes, available) = match found {
            Some(update) => (Some(update.version), update.body, true),
            None => (None, None, false),
        };
        Some(AppUpdateInfo {
            platform: platform_key(),
            current,
            latest,
            notes: notes.filter(|n| !n.is_empty()),
            available,
            versions: Vec::new(),
            source: format!("official:{}", winner.as_str()),
            channel: channel.as_str().to_string(),
            choices: Vec::new(),
        })
    }

    /// 官方 updater 下载并安装（桌面）；调用方负责在成功后重启应用。
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    pub async fn install_official(app: &tauri::AppHandle) -> anyhow::Result<()> {
        use tauri_plugin_updater::UpdaterExt;

        let update = app
            .updater()?
            .check()
            .await?
            .context("官方更新源没有可用版本")?;
        update.download_and_install(|_, _| {}, || {}).await?;
        Ok(())
    }

    /// 打开安装包（桌面：DMG/MSI；Android 由上层走系统安装器）。
    pub fn install_installer(path: &Path) -> anyhow::Result<()> {
        if !path.is_file() {
            bail!("安装包不存在: {}", path.display());
        }

        #[cfg(target_os = "macos")]
        {
            std::process::Command::new("open")
                .arg(path)
                .status()
                .context("打开 DMG 失败")?;
        }
        #[cfg(target_os = "windows")]
        {
            crate::kernel::hidden_command("cmd")
                .args(["/C", "start", ""])
                .arg(path)
                .status()
                .context("启动安装程序失败")?;
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            std::process::Command::new("xdg-open")
                .arg(path)
                .status()
                .context("打开安装包失败")?;
        }
        #[cfg(any(target_os = "android", target_os = "ios"))]
        {
            bail!("移动端请通过系统安装器打开 APK");
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_key_matches_convention() {
        let key = platform_key();
        assert!(
            key.starts_with("macos-")
                || key.starts_with("windows-")
                || key.starts_with("linux-")
                || key.starts_with("android-")
        );
    }

    #[test]
    fn version_orders_by_base_then_stage_then_seq() {
        // 同 base：正式版 > rc > beta > dev > nightly
        let v = Version::parse;
        assert!(v("26.410.20712") > v("26.410.20712-rc.1"));
        assert!(v("26.410.20712-rc.1") > v("26.410.20712-beta.1"));
        assert!(v("26.410.20712-beta.1") > v("26.410.20712-dev.1"));
        assert!(v("26.410.20712-dev.1") > v("26.410.20712-nightly.1115"));
        // 同 base + 同 stage：按 seq
        assert!(v("26.410.20712-dev.2") > v("26.410.20712-dev.1"));
        // 跨 base：base 优先（新 base 的 nightly > 旧 base 的正式版）
        assert!(v("26.410.20713-nightly.1115") > v("26.410.20712"));
    }

    #[test]
    fn legacy_pre_stage_sits_below_nightly() {
        let v = Version::parse;
        assert!(v("26.410.20712-pre.20") < v("26.410.20712-nightly.1115"));
        // 所以 pre.20 用户能收到同 base 的 dev.1（stage 更高）
        assert!(v("26.410.20712-dev.1") > v("26.410.20712-pre.20"));
    }

    #[test]
    fn visibility_is_inclusive_downwards() {
        assert_eq!(Channel::Stable.visible(), vec![Channel::Stable]);
        assert_eq!(
            Channel::Dev.visible(),
            vec![Channel::Stable, Channel::Rc, Channel::Beta, Channel::Dev]
        );
        assert_eq!(Channel::Nightly.visible().len(), 5);
    }

    #[test]
    fn channel_inferred_from_installed_version() {
        assert_eq!(
            Channel::infer_from_version("26.410.20712-pre.20"),
            Channel::Dev
        );
        assert_eq!(
            Channel::infer_from_version("26.410.20713-dev.1"),
            Channel::Dev
        );
        assert_eq!(
            Channel::infer_from_version("26.410.20713-nightly.1115"),
            Channel::Nightly
        );
        assert_eq!(
            Channel::infer_from_version("26.410.20713-rc.1"),
            Channel::Rc
        );
        assert_eq!(Channel::infer_from_version("26.410.20713"), Channel::Stable);
    }

    #[test]
    fn channel_urls_keep_base_and_channel_segment() {
        let base = "https://dl.example/upgrade/";
        assert_eq!(
            channel_manifest_url(base, APP_BASE_URL, Channel::Dev),
            "https://dl.example/upgrade/dev/manifest.json"
        );
        assert_eq!(
            channel_latest_url(base, APP_BASE_URL, Channel::Nightly),
            "https://dl.example/upgrade/nightly/latest.json"
        );
        // 没写结尾斜杠也要归一
        assert_eq!(
            channel_manifest_url("https://dl.example/core", KERNEL_BASE_URL, Channel::Rc),
            "https://dl.example/core/rc/manifest.json"
        );
    }

    #[test]
    fn channel_parse_accepts_aliases() {
        assert_eq!(Channel::parse("PRE"), Some(Channel::Rc));
        assert_eq!(Channel::parse("dev"), Some(Channel::Dev));
        assert_eq!(Channel::parse("bogus"), None);
    }

    #[test]
    fn version_compare_handles_release_scheme() {
        assert!(version_key("26.410.20706") > version_key("26.410.20705"));
        assert!(version_ge("26.410.20706", "26.410.20700"));
        assert!(version_ge("26.410.20706", ""));
        assert!(!version_ge("26.410.20699", "26.410.20700"));
    }

    #[test]
    fn base_url_normalization() {
        assert_eq!(
            normalize_base("https://a.example/core", KERNEL_BASE_URL),
            "https://a.example/core/"
        );
        assert_eq!(normalize_base("", KERNEL_BASE_URL), KERNEL_BASE_URL);
    }

    #[test]
    fn manifest_parses_reference_payload() {
        let raw = r#"{
          "schema": 1,
          "latest": "26.410.20706",
          "versions": [{
            "version": "26.410.20706",
            "rpc_schema": 1,
            "min_studio_version": "26.410.20706",
            "assets": {
              "macos-x86_64": { "file": "macos-x86_64-v26.410.20706.zip", "sha256": "abc", "entry": "exvlan" }
            }
          }]
        }"#;
        let m: Manifest = serde_json::from_str(raw).unwrap();
        assert_eq!(m.schema, 1);
        assert_eq!(m.versions[0].assets["macos-x86_64"].entry, "exvlan");
    }
}
