//! 旧服务定义清理（S4）——**只能留 bin**。
//!
//! `launchctl` / `sc` / `schtasks` 是桌面特权操作，移动端没有对应物；同理
//! `exvlan-migrate` lib 也不该带这些平台分支（Android 也用不上）。
//!
//! 清单与 `exvlan/service/src/main.rs::cleanup_legacy_macos()` / `cleanup_legacy_windows()`
//! 一致（见 [`exvlan_control::paths::LEGACY_LAUNCHD_LABELS`] 等常量）。
//!
//! 语义：**best-effort**
//! - 无权限时整体 `skipped`（提示需要 root/Administrator），不算失败；
//! - 单条失败记 `service_cleanup_partial`，继续下一条。

use exvlan_control::legacy::LegacyService;
use exvlan_migrate::Step;
use serde_json::json;

#[cfg(target_os = "macos")]
use std::path::Path;

/// 清理旧服务定义（S4）。
pub fn cleanup(detected: &[LegacyService], dry_run: bool) -> Step {
    if detected.is_empty() {
        return Step::skipped("service_cleanup", "没有旧服务定义");
    }
    if !is_elevated() {
        // 不是失败：这次运行本来就没有权限，提示用户即可。
        return Step::skipped(
            "service_cleanup",
            "需要管理员（root/Administrator）权限：请用 sudo 重跑，或由 GUI 提权执行",
        )
        .with_detail(json!({
            "pending": detected
                .iter()
                .map(|s| json!({"name": s.name, "kind": s.kind}))
                .collect::<Vec<_>>(),
        }));
    }
    if dry_run {
        return Step::planned("service_cleanup").with_detail(json!({
            "would_remove": detected
                .iter()
                .map(|s| json!({"name": s.name, "kind": s.kind}))
                .collect::<Vec<_>>(),
        }));
    }
    let mut done = Vec::new();
    let mut failed = Vec::new();
    for service in detected {
        match remove_service(service) {
            Ok(()) => done.push(json!({"name": service.name, "kind": service.kind})),
            Err(err) => failed.push(json!({
                "name": service.name,
                "kind": service.kind,
                "error": err,
            })),
        }
    }
    let detail = json!({"removed": done, "failed": failed});
    if failed.is_empty() {
        Step::ok("service_cleanup").with_detail(detail)
    } else if done.is_empty() {
        Step::failed("service_cleanup_partial", "全部旧服务定义清理失败").with_detail(detail)
    } else {
        Step::partial(
            "service_cleanup_partial",
            format!("{} 条失败", failed.len()),
        )
        .with_detail(detail)
    }
}

/// 是否有管理员权限（Unix）。Windows 没有 root 概念，一律尝试后按结果记账。
fn is_elevated() -> bool {
    #[cfg(unix)]
    {
        unsafe { libc::geteuid() == 0 }
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn remove_service(service: &LegacyService) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        use exvlan_control::paths;
        // launchd label：先 bootout（没加载时返回非 0，不算错误），再删同名 plist。
        if paths::LEGACY_LAUNCHD_LABELS.contains(&service.name.as_str()) {
            let _ = std::process::Command::new("/bin/launchctl")
                .args(["bootout", &format!("system/{}", service.name)])
                .output();
            for dir in paths::launchd_dirs() {
                let plist = dir.join(format!("{}.plist", service.name));
                remove_if_exists(&plist)?;
            }
        }
        // plist / 旧 helper 二进制 / helper.sock 都走这一条。
        if let Some(path) = &service.path {
            remove_if_exists(path)?;
        }
        return Ok(());
    }
    #[cfg(target_os = "windows")]
    {
        match service.kind {
            "scm" => {
                let _ = hidden("sc").args(["stop", service.name.as_str()]).output();
                let out = hidden("sc")
                    .args(["delete", service.name.as_str()])
                    .output()
                    .map_err(|err| format!("sc delete 失败: {err}"))?;
                if !out.status.success() {
                    return Err(format!(
                        "sc delete {} 失败: {}",
                        service.name,
                        String::from_utf8_lossy(&out.stderr).trim()
                    ));
                }
            }
            "task" => {
                let out = hidden("schtasks")
                    .args(["/Delete", "/TN", service.name.as_str(), "/F"])
                    .output()
                    .map_err(|err| format!("schtasks 失败: {err}"))?;
                if !out.status.success() {
                    return Err(format!(
                        "schtasks /Delete {} 失败: {}",
                        service.name,
                        String::from_utf8_lossy(&out.stderr).trim()
                    ));
                }
            }
            _ => {}
        }
        return Ok(());
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = service;
        Ok(())
    }
}

#[cfg(target_os = "macos")]
fn remove_if_exists(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    std::fs::remove_file(path).map_err(|err| format!("{}: {err}", path.display()))
}

#[cfg(target_os = "windows")]
fn hidden(program: &str) -> std::process::Command {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    use std::os::windows::process::CommandExt as _;
    let mut cmd = std::process::Command::new(program);
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;
    use exvlan_migrate::Status;

    #[test]
    fn no_legacy_services_is_skipped() {
        let step = cleanup(&[], false);
        assert_eq!(step.status, Status::Skipped);
    }
}
