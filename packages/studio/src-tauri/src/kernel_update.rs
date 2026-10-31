//! 内核在线升级：拉取 `manifest.json` → 校验 → 下载 → 提权安装 → 重启服务。
//!
//! 分发契约（镜像端点）：
//! ```text
//! <base>/manifest.json
//! <base>/<platform>-v<version>.zip
//! ```
//! `manifest.json` 的 `versions[].assets[platform]` 给出文件名、sha256（针对 zip 本体）
//! 与 zip 内的入口文件名。

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

/// 默认分发地址（可通过设置页覆盖）。
pub const DEFAULT_BASE_URL: &str = "https://dl.exvlan.era-x.cn/core/";

/// 支持该 manifest schema 的最低/最高版本。
const MANIFEST_SCHEMA: u32 = 1;

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
    #[serde(default = "default_entry")]
    pub entry: String,
}

fn default_entry() -> String {
    super::kernel::kernel_bin_name().to_string()
}

/// 当前平台在 manifest 里的键名，如 `macos-x86_64` / `windows-x86_64` / `linux-aarch64`。
pub fn platform_key() -> String {
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

fn normalize_base(base: &str) -> String {
    let trimmed = base.trim();
    let trimmed = if trimmed.is_empty() {
        DEFAULT_BASE_URL
    } else {
        trimmed
    };
    if trimmed.ends_with('/') {
        trimmed.to_string()
    } else {
        format!("{trimmed}/")
    }
}

/// 解析 `YY.QMM.Mmpp` 形式的版本号，用于比较。
fn version_key(v: &str) -> Vec<u64> {
    v.trim()
        .trim_start_matches('v')
        .split(['.', '-', '+'])
        .filter_map(|s| s.parse::<u64>().ok())
        .collect()
}

fn version_ge(a: &str, b: &str) -> bool {
    if b.trim().is_empty() {
        return true;
    }
    version_key(a) >= version_key(b)
}

async fn fetch_manifest(base: &str) -> anyhow::Result<Manifest> {
    let base = normalize_base(base);
    let url = format!("{base}manifest.json");
    let resp = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()?
        .get(&url)
        .send()
        .await
        .with_context(|| format!("请求 {url} 失败"))?;
    if !resp.status().is_success() {
        bail!("{} 返回 {}", url, resp.status());
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

/// 升级检查结果（给前端）。
#[derive(Debug, Clone, Serialize)]
pub struct UpdateInfo {
    pub platform: String,
    pub current: Option<String>,
    pub latest: Option<String>,
    pub available: bool,
    /// 可安装的版本列表（新→旧），用于回滚。
    pub versions: Vec<String>,
    pub source: String,
}

fn pick_entry<'a>(manifest: &'a Manifest, version: &str) -> Option<&'a VersionEntry> {
    manifest.versions.iter().find(|v| v.version == version)
}

/// 检查更新：返回最新版本与完整版本列表。
pub async fn check_update(base: &str, studio_version: &str) -> anyhow::Result<UpdateInfo> {
    let manifest = fetch_manifest(base).await?;
    let platform = platform_key();
    let current = super::kernel::installed_version();

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
        source: normalize_base(base),
    })
}

/// 下载指定版本并安装（会把内核放进系统目录，需一次授权）。
pub async fn install_version(
    base: &str,
    version: &str,
    studio_version: &str,
) -> anyhow::Result<()> {
    let base = normalize_base(base);
    let manifest = fetch_manifest(&base).await?;
    let platform = platform_key();

    let entry =
        pick_entry(&manifest, version).with_context(|| format!("manifest 中没有版本 {version}"))?;
    if entry.rpc_schema != 0 && entry.rpc_schema > 1 {
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
            bail!(
                "内核包校验失败：期望 {}，实际 {}（已丢弃）",
                asset.sha256,
                actual
            );
        }
    }

    let tmp = tempfile::tempdir().context("创建临时目录失败")?;
    let archive = tmp.path().join("kernel.zip");
    std::fs::write(&archive, &bytes).context("写入临时文件失败")?;

    let binary = tmp.path().join(&asset.entry);
    extract_entry(&archive, &asset.entry, &binary)?;

    super::run_elevated(&["--install-kernel", &binary.display().to_string(), version])
        .map_err(|e| anyhow::anyhow!(e))?;

    // 重启服务让新内核生效（失败不视为安装失败，UI 会提示需要手动重启）。
    let _ = super::run_elevated(&["--service-ctl", "stop"]);
    if let Err(err) = super::run_elevated(&["--service-ctl", "start"]) {
        eprintln!("[kernel] 重启服务失败（内核已安装，可手动重启）: {err}");
    }

    Ok(())
}

/// 从 zip 中解出入口二进制。
fn extract_entry(archive: &Path, entry: &str, out: &Path) -> anyhow::Result<()> {
    let file = std::fs::File::open(archive).context("打开内核包失败")?;
    let mut zip = zip::ZipArchive::new(file).context("解析内核包失败")?;

    // 兼容 zip 根目录与单层目录两种打包方式。
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
    zf.read_to_end(&mut buf)?;
    std::fs::write(out, &buf).context("写出内核二进制失败")?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(out, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

/// 当前可用的分发地址（环境变量优先，其次默认值）。
pub fn base_url_override() -> String {
    std::env::var("EXVLAN_KERNEL_BASE_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
}

/// 供调试/测试使用：临时目录路径辅助。
pub fn temp_file(name: &str) -> PathBuf {
    std::env::temp_dir().join(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_key_matches_manifest_convention() {
        let key = platform_key();
        assert!(
            key.starts_with("macos-") || key.starts_with("windows-") || key.starts_with("linux-")
        );
        assert!(key.ends_with("x86_64") || key.ends_with("aarch64") || key.ends_with("arm"));
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
            normalize_base("https://a.example/core"),
            "https://a.example/core/"
        );
        assert_eq!(
            normalize_base("https://a.example/core/"),
            "https://a.example/core/"
        );
        assert_eq!(normalize_base(""), DEFAULT_BASE_URL);
        assert!(normalize_base("  ").ends_with('/'));
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
