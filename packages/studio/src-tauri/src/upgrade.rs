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
pub fn platform_key() -> String {
    if cfg!(target_os = "android") || cfg!(target_os = "ios") {
        let abi = if cfg!(target_arch = "aarch64") {
            "arm64-v8a"
        } else if cfg!(target_arch = "arm") {
            "armeabi-v7a"
        } else if cfg!(target_arch = "x86") {
            "x86"
        } else {
            "x86_64"
        };
        return format!("android-{abi}");
    }

    let os = if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "linux"
    };
    let arch = if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "arm") {
        "arm"
    } else {
        "unknown"
    };
    format!("{os}-{arch}")
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
    pub const LOCAL_RPC_HINT: &str = "tcp://127.0.0.1:15999";

    #[derive(Debug, Clone, Serialize)]
    pub struct UpdateInfo {
        pub platform: String,
        pub current: Option<String>,
        pub latest: Option<String>,
        pub available: bool,
        pub versions: Vec<String>,
        pub source: String,
    }

    /// 检查内核更新（移动端不支持：内核随包分发）。
    pub async fn check_update(base: &str, studio_version: &str) -> anyhow::Result<UpdateInfo> {
        #[cfg(any(target_os = "android", target_os = "ios"))]
        {
            let _ = (base, studio_version);
            bail!("移动端内核随应用发布，不支持在线升级");
        }

        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        {
            let base = normalize_base(base, KERNEL_BASE_URL);
            let manifest = fetch_manifest(&base, KERNEL_BASE_URL).await?;
            let platform = platform_key();
            let current = crate::kernel::installed_version();

            let mut versions: Vec<String> = manifest
                .versions
                .iter()
                .filter(|v| v.assets.contains_key(&platform))
                .filter(|v| version_ge(studio_version, &v.min_studio_version))
                .map(|v| v.version.clone())
                .collect();
            versions.sort_by_key(|v| version_key(v));
            versions.reverse();

            let latest = versions
                .first()
                .cloned()
                .or_else(|| (!manifest.latest.is_empty()).then(|| manifest.latest.clone()));

            let available = match (&current, &latest) {
                (None, Some(_)) => true,
                (Some(c), Some(l)) => version_key(l) > version_key(c),
                _ => false,
            };

            Ok(UpdateInfo {
                platform,
                current,
                latest,
                available,
                versions,
                source: base,
            })
        }
    }

    /// 下载并安装指定内核版本（需一次授权）。
    pub async fn install_version(
        base: &str,
        version: &str,
        studio_version: &str,
    ) -> anyhow::Result<()> {
        #[cfg(any(target_os = "android", target_os = "ios"))]
        {
            let _ = (base, version, studio_version);
            bail!("移动端内核随应用发布，不支持在线升级");
        }

        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        {
            let base = normalize_base(base, KERNEL_BASE_URL);
            let manifest = fetch_manifest(&base, KERNEL_BASE_URL).await?;
            let platform = platform_key();

            let entry = pick(&manifest, version)
                .with_context(|| format!("manifest 中没有版本 {version}"))?;
            if entry.rpc_schema > 1 {
                bail!(
                    "内核 {version} 的 RPC 契约版本为 {}，当前 GUI 不支持（请先升级客户端）",
                    entry.rpc_schema
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
                .with_context(|| format!("内核 {version} 未提供 {platform} 平台的包"))?;

            let bytes = download_verified(&base, asset).await?;

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
    }

    fn current_version() -> String {
        env!("CARGO_PKG_VERSION").to_string()
    }

    /// 检查应用更新。
    pub async fn check_update(base: &str) -> anyhow::Result<AppUpdateInfo> {
        let base = normalize_base(base, APP_BASE_URL);
        let manifest = fetch_manifest(&base, APP_BASE_URL).await?;
        let platform = platform_key();
        let current = current_version();

        let mut versions: Vec<String> = manifest
            .versions
            .iter()
            .filter(|v| v.assets.contains_key(&platform))
            .map(|v| v.version.clone())
            .collect();
        versions.sort_by_key(|v| version_key(v));
        versions.reverse();

        let entry = versions
            .first()
            .and_then(|v| pick(&manifest, v).cloned())
            .or_else(|| pick(&manifest, &manifest.latest).cloned());

        let latest = entry.as_ref().map(|e| e.version.clone());
        let available = latest
            .as_ref()
            .map(|l| version_key(l) > version_key(&current))
            .unwrap_or(false);

        Ok(AppUpdateInfo {
            platform,
            current,
            latest,
            notes: entry.map(|e| e.notes).filter(|n| !n.is_empty()),
            available,
            versions,
            source: base,
        })
    }

    /// 下载安装包到临时目录（已校验 sha256），返回路径。
    pub async fn download_installer(base: &str, version: &str) -> anyhow::Result<PathBuf> {
        let base = normalize_base(base, APP_BASE_URL);
        let manifest = fetch_manifest(&base, APP_BASE_URL).await?;
        let platform = platform_key();
        let entry =
            pick(&manifest, version).with_context(|| format!("manifest 中没有版本 {version}"))?;
        let asset = entry
            .assets
            .get(&platform)
            .with_context(|| format!("应用 {version} 未提供 {platform} 平台的安装包"))?;

        let bytes = download_verified(&base, asset).await?;
        let dir = std::env::temp_dir().join("exvlan-update");
        std::fs::create_dir_all(&dir).context("创建更新临时目录失败")?;
        let path = dir.join(&asset.file);
        std::fs::write(&path, &bytes).context("写入安装包失败")?;
        Ok(path)
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
            std::process::Command::new("cmd")
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
