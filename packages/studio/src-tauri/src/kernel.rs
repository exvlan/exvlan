//! 内核二进制仓库：GUI 只负责把内核装到系统位置并交给服务管理器，
//! 自身不再链接内核。
//!
//! 落盘位置（root-owned，普通用户不可写）：
//! - macOS:  `/Library/Application Support/exvlan/core/exvlan`
//! - Windows: `C:\ProgramData\exvlan\core\exvlan.exe`
//! - Linux:   `/opt/exvlan/core/exvlan`
//!
//! 同目录下的 `version` 记录当前版本号（纯文本）。

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, bail};

/// 内核可执行文件名。
pub fn kernel_bin_name() -> &'static str {
    if cfg!(windows) {
        "exvlan.exe"
    } else {
        "exvlan"
    }
}

/// 内核仓库目录（需 root 写入）。
pub fn kernel_dir() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        PathBuf::from("/Library/Application Support/exvlan/core")
    }
    #[cfg(target_os = "windows")]
    {
        PathBuf::from(r"C:\ProgramData\exvlan\core")
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        PathBuf::from("/opt/exvlan/core")
    }
}

/// 当前内核二进制路径（服务管理器应当指向这里）。
pub fn kernel_path() -> PathBuf {
    kernel_dir().join(kernel_bin_name())
}

fn version_path() -> PathBuf {
    kernel_dir().join("version")
}

/// 是否已安装内核。
pub fn is_installed() -> bool {
    kernel_path().is_file()
}

/// 已安装内核的版本号（优先读 `version` 文件，失败则问二进制）。
pub fn installed_version() -> Option<String> {
    if let Ok(text) = std::fs::read_to_string(version_path()) {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    let out = Command::new(kernel_path()).arg("--version").output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    // 形如 "exvlan 26.410.20706-..."，取第二段。
    text.split_whitespace()
        .nth(1)
        .map(|s| s.split('-').next().unwrap_or(s).to_string())
}

/// 把 `src` 安装为当前内核。**需要 root**（由提权子命令调用）。
///
/// 流程：复制到 `<core>/exvlan.new` → 预演执行 `--version` 校验可运行 →
/// 原子 rename 覆盖 → 写版本号 → 收紧权限。任一步失败都不会破坏现有内核。
pub fn install_kernel(src: &Path, version: &str) -> anyhow::Result<()> {
    if !src.is_file() {
        bail!("内核源文件不存在: {}", src.display());
    }

    let dir = kernel_dir();
    std::fs::create_dir_all(&dir).with_context(|| format!("无法创建内核目录 {}", dir.display()))?;

    let staged = dir.join(format!("{}.new", kernel_bin_name()));
    std::fs::copy(src, &staged).with_context(|| format!("复制内核到 {} 失败", staged.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))?;
    }

    // 预演：先把旧内核放一边，验证新二进制能跑（架构/签名/依赖）。
    let output = Command::new(&staged)
        .arg("--version")
        .output()
        .with_context(|| {
            format!(
                "预演执行 {} 失败（可能架构不匹配或未签名）",
                staged.display()
            )
        })?;
    if !output.status.success() {
        let _ = std::fs::remove_file(&staged);
        bail!(
            "新内核实跑校验失败（exit={}）：{}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    std::fs::rename(&staged, kernel_path())
        .with_context(|| format!("替换内核 {} 失败", kernel_path().display()))?;
    std::fs::write(version_path(), format!("{version}\n"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755))?;
        std::fs::set_permissions(kernel_path(), std::fs::Permissions::from_mode(0o755))?;
    }

    Ok(())
}

/// 内置内核（随 app 打包的资源）路径。
///
/// 约定：`<app resources>/kernel/{exvlan,exvlan.exe}`，由 CI 在打包时放入
/// （内核二进制来自 core 构建产物，不再由 GUI 编译）。
pub fn bundled_kernel(app: &tauri::AppHandle) -> Option<PathBuf> {
    use tauri::Manager as _;
    let dir = app.path().resource_dir().ok()?;
    let candidate = dir.join("kernel").join(kernel_bin_name());
    if candidate.is_file() {
        Some(candidate)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kernel_paths_are_absolute_and_stable() {
        let dir = kernel_dir();
        assert!(dir.is_absolute());
        assert!(dir.ends_with("core"));
        assert_eq!(kernel_path(), dir.join(kernel_bin_name()));
    }

    #[test]
    fn bin_name_matches_platform() {
        if cfg!(windows) {
            assert_eq!(kernel_bin_name(), "exvlan.exe");
        } else {
            assert_eq!(kernel_bin_name(), "exvlan");
        }
    }

    #[cfg(unix)]
    #[test]
    fn install_rejects_missing_source() {
        let err = install_kernel(Path::new("/nonexistent/exvlan"), "0").unwrap_err();
        assert!(err.to_string().contains("不存在"));
    }
}
