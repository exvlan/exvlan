//! 旧路径的清理（S5）+ 删源，全部 **best-effort**。
//!
//! - 路径清单只有一份：[`exvlan_control::paths::stale_paths`]（GUI 安装完服务后
//!   也用同一份），这里逐条删、逐条记账；
//! - 单条失败只记 `stale_paths` 的 `failed`，继续下一条；
//! - 旧**服务定义**（launchd / SCM / 计划任务）是桌面特权操作，留在 bin
//!   （`main.rs::legacy_services`），移动端与 lib 都不带这些平台分支。

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
        let step = remove_sources(
            std::slice::from_ref(&networks),
            std::slice::from_ref(&ui),
            false,
            false,
        );
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
        let step = remove_sources(&[], std::slice::from_ref(&ui), true, false);
        assert_eq!(step.status, Status::Ok);
        assert!(!ui.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dry_run_reports_without_deleting() {
        let dir = temp("dry");
        let ui = dir.join("ui-state.json");
        std::fs::write(&ui, "{}").unwrap();
        let step = remove_sources(&[], std::slice::from_ref(&ui), true, true);
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
}
