//! 内核二进制仓库：GUI 只负责把内核装到系统位置并交给服务管理器，
//! 自身不再链接内核。
//!
//! 落盘位置（root-owned，普通用户不可写）：
//! - macOS:  `/Library/Application Support/era.exvlan.studio/core/exvlan`
//! - Windows: `C:\ProgramData\era.exvlan.studio\core\exvlan.exe`
//! - Linux:   `/opt/era.exvlan.studio/core/exvlan`
//!
//! 同目录下的 `version` 记录当前版本号（纯文本）。

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, bail};

/// Windows：控制台程序被 GUI（无控制台）拉起时，系统会新建一个 conhost 窗口。
/// 只重定向 stdout/stderr 不够，必须显式传创建标志（clash-verge-rev / Zephyr 同做法）。
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 拉起控制台子系统程序（内核 / sc / schtasks / curl / cmd …）的统一入口。
///
/// Windows 上 GUI 进程没有控制台，系统会为每个控制台子进程新建 conhost 窗口；
/// 只重定向 stdout/stderr 不够，必须显式传创建标志（clash-verge-rev / Zephyr 同做法）。
pub fn hidden_command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// 拉起内核进程。所有 `exvlan(.exe)` 子进程都必须走这里，否则 Windows 上会闪黑框。
pub fn kernel_command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    hidden_command(program)
}

/// 内核可执行文件名。
pub fn kernel_bin_name() -> &'static str {
    if cfg!(windows) {
        "exvlan.exe"
    } else {
        "exvlan"
    }
}

/// Windows 内核运行时依赖（必须与 `exvlan.exe` 同目录）。
///
/// 抽成纯函数方便在非 Windows 上单测文件名规则。
pub fn runtime_dep_names(is_64bit: bool) -> Vec<&'static str> {
    vec![
        "Packet.dll",
        "wintun.dll",
        if is_64bit {
            "WinDivert64.sys"
        } else {
            "WinDivert32.sys"
        },
    ]
}

/// 当前平台上内核运行时依赖的落地情况（诊断用）。
pub fn runtime_dep_status() -> Vec<(String, bool)> {
    if !cfg!(windows) {
        return Vec::new();
    }
    let dir = kernel_dir();
    runtime_dep_names(cfg!(target_pointer_width = "64"))
        .into_iter()
        .map(|name| (name.to_string(), dir.join(name).is_file()))
        .collect()
}

#[cfg(windows)]
fn install_runtime_deps(src: &Path) -> anyhow::Result<()> {
    let dir = kernel_dir();
    // 源目录优先级：
    //   1) kernel_src 的父目录 —— 便携包 / 在线更新解压后 DLL 就在 exe 旁边
    //   2) 当前 GUI 可执行文件所在目录 —— MSI 安装到 Program Files 时 DLL 在应用根
    let mut search: Vec<PathBuf> = Vec::new();
    if let Some(parent) = src.parent() {
        search.push(parent.to_path_buf());
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(parent) = exe.parent()
    {
        search.push(parent.to_path_buf());
    }
    search.dedup();

    let mut missing: Vec<String> = Vec::new();
    for name in runtime_dep_names(cfg!(target_pointer_width = "64")) {
        let target = dir.join(name);
        if target.is_file() {
            continue;
        }
        let found = search.iter().map(|d| d.join(name)).find(|p| p.is_file());
        match found {
            Some(from) => {
                std::fs::copy(&from, &target).with_context(|| {
                    format!("复制依赖 {} -> {} 失败", from.display(), target.display())
                })?;
                crate::logger::record(
                    "kernel.deps",
                    format!("copied {} from {}", name, from.display()),
                );
            }
            None => missing.push(name.to_string()),
        }
    }

    if !missing.is_empty() {
        // 提前失败，避免用户只看到系统的 STATUS_DLL_NOT_FOUND (0xc0000135) 弹窗。
        bail!(
            "内核缺少运行时依赖: {}（查找目录: {}）。请确认安装包完整，或把依赖放到 {}",
            missing.join(", "),
            search
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", "),
            dir.display()
        );
    }
    Ok(())
}

#[cfg(not(windows))]
fn install_runtime_deps(_src: &Path) -> anyhow::Result<()> {
    Ok(())
}

/// 内核仓库目录（需 root 写入）。
pub fn kernel_dir() -> PathBuf {
    crate::paths::system().core
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
    let out = kernel_command(kernel_path())
        .arg("--version")
        .output()
        .ok()?;
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

    // Windows：先把 Packet.dll / wintun.dll / WinDivert*.sys 放到位，
    // 否则下面的 `--version` 预演会直接以 STATUS_DLL_NOT_FOUND (0xc0000135) 失败。
    install_runtime_deps(src)?;

    let staged = dir.join(format!("{}.new", kernel_bin_name()));
    std::fs::copy(src, &staged).with_context(|| format!("复制内核到 {} 失败", staged.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))?;
    }

    // 预演：先把旧内核放一边，验证新二进制能跑（架构/签名/依赖）。
    let output = kernel_command(&staged)
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
        // root 守护进程会执行这个二进制：必须是 root:wheel，否则本地任意用户可替换 → 提权。
        set_root_owner(&dir)?;
        set_root_owner(&kernel_path())?;
        set_root_owner(&version_path())?;
    }

    verify_root_owned(&kernel_path())?;

    #[cfg(target_os = "linux")]
    {
        // 普通模式（用户态子进程）要能建 TUN：给内核二进制授予
        // CAP_NET_ADMIN / CAP_NET_RAW。每次替换内核后都要重做（新 inode 不继承旧 capability）。
        match std::process::Command::new("setcap")
            .args(["cap_net_admin,cap_net_raw+ep"])
            .arg(kernel_path())
            .status()
        {
            Ok(status) if status.success() => {
                crate::logger::record("kernel.setcap", "cap_net_admin,cap_net_raw granted");
            }
            Ok(status) => {
                crate::logger::record("kernel.setcap", format!("setcap exited with {status}"))
            }
            Err(err) => crate::logger::record(
                "kernel.setcap",
                format!("setcap 不可用（普通模式 TUN 将需要提权）: {err}"),
            ),
        }
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
    let bin = kernel_bin_name();

    // Tauri 会保留 `bundle.resources` 里的相对路径：
    //   macOS  → <App>.app/Contents/Resources/resources/kernel/exvlan
    //   Windows→ <install dir>/resources/kernel/exvlan.exe
    // 但也可能因打包方式被摊平，这里按优先级全部试一遍。
    let candidates = [
        dir.join("kernel").join(bin),
        dir.join("resources").join("kernel").join(bin),
        dir.join(bin),
    ];
    for path in &candidates {
        if path.is_file() {
            return Some(path.clone());
        }
    }

    // 兜底：在资源目录下有限深度搜索（避免打包布局变化导致彻底不可用）。
    match find_named(&dir, bin, 4) {
        Some(path) => Some(path),
        None => {
            eprintln!(
                "[kernel] 未找到随包内核 {}；resource_dir={}，已尝试: {:?}",
                bin,
                dir.display(),
                candidates
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
            );
            None
        }
    }
}

/// 在 `root` 下有限深度查找指定文件名（用于兼容不同打包布局）。
fn find_named(root: &Path, name: &str, max_depth: usize) -> Option<PathBuf> {
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        let entries = std::fs::read_dir(&dir).ok()?;
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_dir() {
                if depth < max_depth {
                    stack.push((path, depth + 1));
                }
            } else if path.file_name().and_then(|n| n.to_str()) == Some(name) {
                return Some(path);
            }
        }
    }
    None
}

/// 把路径属主改为 `root:wheel`（仅 unix；调用方应已是 root）。
#[cfg(unix)]
fn set_root_owner(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::ffi::OsStrExt as _;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| anyhow::anyhow!("路径包含 NUL: {}", path.display()))?;
    // uid=0, gid=0（macOS 的 wheel 即为 0）
    let rc = unsafe { libc::chown(c.as_ptr(), 0, 0) };
    if rc != 0 {
        bail!(
            "chown root:wheel 失败（{}）: {}",
            path.display(),
            std::io::Error::last_os_error()
        );
    }
    Ok(())
}

/// 校验内核二进制的属主是 root，避免 root 执行用户可写文件。
#[cfg(unix)]
fn verify_root_owned(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::MetadataExt as _;
    let meta =
        std::fs::metadata(path).with_context(|| format!("读取 {} 元数据失败", path.display()))?;
    if meta.uid() != 0 {
        bail!(
            "内核属主不是 root（uid={}）：{} —— 拒绝以 root 执行用户可写文件",
            meta.uid(),
            path.display()
        );
    }
    Ok(())
}

#[cfg(not(unix))]
fn verify_root_owned(_path: &Path) -> anyhow::Result<()> {
    Ok(())
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

    #[test]
    fn runtime_dep_names_follow_arch() {
        let d64 = runtime_dep_names(true);
        assert!(d64.contains(&"Packet.dll"));
        assert!(d64.contains(&"wintun.dll"));
        assert!(d64.contains(&"WinDivert64.sys"));

        let d32 = runtime_dep_names(false);
        assert!(d32.contains(&"WinDivert32.sys"));
        assert!(!d32.contains(&"WinDivert64.sys"));
    }

    #[cfg(unix)]
    #[test]
    fn install_rejects_missing_source() {
        let err = install_kernel(Path::new("/nonexistent/exvlan"), "0").unwrap_err();
        assert!(err.to_string().contains("不存在"));
    }
}
