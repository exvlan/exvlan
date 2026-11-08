//! `era.exvlan.studio.helper` 的 GUI 侧封装（仅 macOS）。
//!
//! 首次安装由一次提权完成（同一次里装内核 + 服务 + helper）；之后的 App 升级
//! 只需要把「已签名的内核」交给常驻 helper，**不再弹授权**。

#![allow(dead_code)]

/// 必须与 `packages/helper` 的 `HELPER_VERSION` 保持一致。
pub const HELPER_VERSION: u32 = 1;

#[cfg(target_os = "macos")]
mod imp {
    use super::*;
    use anyhow::{Context as _, bail};
    use tauri::Manager as _;

    const HELPER_BIN: &str = "/Library/PrivilegedHelperTools/era.exvlan.studio.helper";
    const HELPER_VERSION_FILE: &str =
        "/Library/PrivilegedHelperTools/era.exvlan.studio.helper.version";

    /// 已安装且版本匹配。
    pub fn is_installed() -> bool {
        if !Path::new(HELPER_BIN).is_file() {
            return false;
        }
        let text = std::fs::read_to_string(HELPER_VERSION_FILE).unwrap_or_default();
        text.split_whitespace()
            .next()
            .and_then(|v| v.parse::<u32>().ok())
            == Some(super::HELPER_VERSION)
    }

    /// 随包 helper 的路径。
    pub fn bundled_helper(app: &tauri::AppHandle) -> Option<PathBuf> {
        let dir = app.path().resource_dir().ok()?;
        let candidates = [
            dir.join("helper").join("exvlan-helper"),
            dir.join("resources").join("helper").join("exvlan-helper"),
            dir.join("exvlan-helper"),
        ];
        candidates.into_iter().find(|p| p.is_file())
    }

    /// 安装 helper（由提权子进程以 root 调用；`src` 是随包 helper 的绝对路径）。
    pub fn install_from(src: &Path, owner_uid: u32) -> anyhow::Result<()> {
        if !src.is_file() {
            bail!("随包 helper 不存在: {}", src.display());
        }
        let out = std::process::Command::new(src)
            .arg("--install")
            .arg(owner_uid.to_string())
            .output()
            .with_context(|| format!("执行 {} --install 失败", src.display()))?;
        if !out.status.success() {
            bail!(
                "helper 安装失败: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(())
    }

    /// 把随包的已签名内核交给 helper（当前用户执行，无需提权）。
    pub fn apply_bundled(app: &tauri::AppHandle) -> anyhow::Result<()> {
        if !is_installed() {
            bail!("helper 未安装");
        }
        let kernel = crate::kernel::bundled_kernel(app).context("未找到随包内核")?;
        let sig = kernel.with_extension("sig");
        if !sig.is_file() {
            bail!("缺少内核签名 {}", sig.display());
        }

        let staging = std::env::temp_dir().join(format!(
            "era-exvlan-update-{}-{}",
            std::process::id(),
            crate::error_log::timestamp().replace(':', "-")
        ));
        std::fs::create_dir_all(&staging)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o700))?;
        }
        std::fs::copy(&kernel, staging.join("kernel"))?;
        std::fs::copy(&sig, staging.join("kernel.sig"))?;
        std::fs::write(staging.join("version"), env!("CARGO_PKG_VERSION"))?;

        let out = std::process::Command::new(HELPER_BIN)
            .arg("--apply")
            .arg(&staging)
            .output()
            .with_context(|| format!("执行 {HELPER_BIN} --apply 失败"))?;
        let _ = std::fs::remove_dir_all(&staging);
        if !out.status.success() {
            bail!(
                "helper apply 失败: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(())
    }

    /// 查询 helper 状态（诊断用）。
    pub fn status() -> Result<String, String> {
        if !is_installed() {
            return Ok("not installed".to_string());
        }
        let out = std::process::Command::new(HELPER_BIN)
            .arg("--status")
            .output()
            .map_err(|e| format!("执行 helper --status 失败: {e}"))?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use std::path::{Path, PathBuf};

    pub fn is_installed() -> bool {
        false
    }

    pub fn bundled_helper(_app: &tauri::AppHandle) -> Option<PathBuf> {
        None
    }

    pub fn install_from(_src: &Path, _owner_uid: u32) -> anyhow::Result<()> {
        anyhow::bail!("helper 仅支持 macOS")
    }

    pub fn apply_bundled(_app: &tauri::AppHandle) -> anyhow::Result<()> {
        anyhow::bail!("helper 仅支持 macOS")
    }

    pub fn status() -> Result<String, String> {
        Ok("unsupported".to_string())
    }
}

#[allow(unused_imports)]
pub use imp::{apply_bundled, bundled_helper, install_from, is_installed, status};

/// 当前进程 uid（供提权安装使用）。
pub fn current_uid() -> u32 {
    #[cfg(unix)]
    {
        unsafe { libc::getuid() }
    }
    #[cfg(not(unix))]
    {
        0
    }
}
