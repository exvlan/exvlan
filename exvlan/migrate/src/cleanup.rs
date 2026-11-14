//! 旧路径 / 旧服务定义的清理（S5 + S4），全部 **best-effort**。
//!
//! - 路径清单只有一份：[`exvlan_control::paths::stale_paths`]（GUI 安装完服务后
//!   也用同一份），这里逐条删、逐条记账；
//! - 旧服务定义（macOS launchd label / Windows SCM 服务与计划任务）需要
//!   root/admin：无权限时整体降级为「报告 + 提示需要管理员」，**不 bail**；
//! - 单条失败只记 `service_cleanup_partial` / `stale_paths` 的 `failed`，继续下一条。

use std::path::{Path, PathBuf};

use serde_json::json;

use crate::report::{Status, Step};

/// 删除清单里的存量路径（存在的才动）。
pub fn stale_paths(paths: &[PathBuf], dry_run: bool) -> Step {
    if paths.is_empty() {
        return Step::skipped("stale_paths", "没有 7.2 及更早的路径残留");
    }
    let mut removed = Vec::new();
    let mut failed = Vec::new();
    let mut missing = 0usize;
    for path in paths {
        if !path.exists() {
            missing += 1;
            continue;
        }
        if dry_run {
            removed.push(path.display().to_string());
            continue;
        }
        match remove_path(path) {
            Ok(()) => removed.push(path.display().to_string()),
            Err(err) => failed.push(json!({
                "path": path.display().to_string(),
                "error": err.to_string(),
            })),
        }
    }
    let detail = json!({
        "removed": removed,
        "failed": failed,
        "missing": missing,
    });
    if dry_run {
        return Step::planned("stale_paths").with_detail(detail);
    }
    if failed.is_empty() {
        Step::ok("stale_paths").with_detail(detail)
    } else {
        Step::partial(
            "stale_paths",
            format!("{} 条删除失败（其余已删）", failed.len()),
        )
        .with_detail(detail)
    }
}

fn remove_path(path: &Path) -> std::io::Result<()> {
    if path.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
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

/// 清理旧服务定义（S4）。
///
/// 清单与 `exvlan/service/src/main.rs::cleanup_legacy_macos()` / `cleanup_legacy_windows()`
/// 一致（见 [`exvlan_control::paths::LEGACY_LAUNCHD_LABELS`] 等常量）。
pub fn legacy_services(detected: &[exvlan_control::legacy::LegacyService], dry_run: bool) -> Step {
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

fn remove_service(service: &exvlan_control::legacy::LegacyService) -> Result<(), String> {
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

/// 迁移完成后删源（`networks.toml` / `service.json` / `config.d` / `.prev` /
/// **条件删除** `ui-state.json*`）。
///
/// `ui_state_ok == false` 时保留 `ui-state.json*`：prefs 没写成功就删源 = 丢数据。
pub fn remove_sources(
    sources: &[PathBuf],
    ui_state: &[PathBuf],
    ui_state_ok: bool,
    dry_run: bool,
) -> Step {
    let mut removed = Vec::new();
    let mut kept = Vec::new();
    let mut failed = Vec::new();
    let mut todo: Vec<&PathBuf> = sources.iter().collect();
    if ui_state_ok {
        todo.extend(ui_state.iter());
    } else {
        kept.extend(ui_state.iter().map(|p| p.display().to_string()));
    }
    if dry_run {
        return Step::planned("cleanup").with_detail(json!({
            "would_remove": todo.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
            "kept": kept,
        }));
    }
    for path in todo {
        if !path.exists() {
            continue;
        }
        match remove_path(path) {
            Ok(()) => removed.push(path.display().to_string()),
            Err(err) => failed.push(json!({
                "path": path.display().to_string(),
                "error": err.to_string(),
            })),
        }
    }
    let detail = json!({"removed": removed, "kept": kept, "failed": failed});
    if !failed.is_empty() {
        Step::partial("cleanup", format!("{} 条删除失败", failed.len())).with_detail(detail)
    } else if removed.is_empty() && kept.is_empty() {
        Step::skipped("cleanup", "没有需要删除的源文件")
    } else {
        Step::new("cleanup", Status::Ok).with_detail(detail)
    }
}

/// 供 `main` 汇总报告用：一次性删掉所有「源」并返回结论。
pub fn source_step(
    legacy: &exvlan_control::legacy::Legacy,
    ui_state_ok: bool,
    dry_run: bool,
) -> Step {
    let sources: Vec<PathBuf> = [
        legacy.networks_toml.clone(),
        legacy.service_json.clone(),
        legacy.config_d.clone(),
    ]
    .into_iter()
    .flatten()
    .chain(legacy.prev_kernels.iter().cloned())
    .collect();
    remove_sources(&sources, &legacy.ui_state, ui_state_ok, dry_run)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "exvlan-migrate-clean-{tag}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn keeps_ui_state_when_prefs_not_ok() {
        let dir = temp("keep-ui");
        let ui = dir.join("ui-state.json");
        let networks = dir.join("networks.toml");
        std::fs::write(&ui, "{}").unwrap();
        std::fs::write(&networks, "").unwrap();
        let step = remove_sources(&[networks.clone()], &[ui.clone()], false, false);
        assert_eq!(step.status, Status::Ok);
        assert!(ui.is_file(), "prefs 没写成功就不能删 ui-state.json");
        assert!(!networks.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn deletes_ui_state_when_prefs_ok() {
        let dir = temp("del-ui");
        let ui = dir.join("ui-state.json");
        std::fs::write(&ui, "{}").unwrap();
        let step = remove_sources(&[], &[ui.clone()], true, false);
        assert_eq!(step.status, Status::Ok);
        assert!(!ui.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dry_run_reports_without_deleting() {
        let dir = temp("dry");
        let ui = dir.join("ui-state.json");
        std::fs::write(&ui, "{}").unwrap();
        let step = remove_sources(&[], &[ui.clone()], true, true);
        assert_eq!(step.status, Status::Planned);
        assert!(ui.is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stale_paths_reports_each_entry() {
        let dir = temp("stale");
        let old_dir = dir.join("old");
        std::fs::create_dir_all(old_dir.join("nested")).unwrap();
        std::fs::write(old_dir.join("nested").join("f"), "x").unwrap();
        let missing = dir.join("missing");
        let step = stale_paths(&[old_dir.clone(), missing], false);
        assert_eq!(step.status, Status::Ok);
        assert!(!old_dir.exists());
        assert_eq!(step.detail["missing"], 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_legacy_services_is_skipped() {
        let step = legacy_services(&[], false);
        assert_eq!(step.status, Status::Skipped);
    }
}
