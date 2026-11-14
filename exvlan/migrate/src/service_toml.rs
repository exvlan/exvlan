//! `service.toml` 归一（S8）：顶层 `mode` → `policy`，旧词汇 → 新词汇。
//!
//! 规则与判定口径都在 [`exvlan_control::legacy::service_toml`]（GUI 闸门 / doctor /
//! 本工具共用同一份实现）。这里只负责读、落盘、记账。
//!
//! 落盘方式与 `sanitize_service_config` 一致：临时文件 + rename（服务可能正在读同一份
//! 文件；原文件是 root 0600）。归一后 `docs/decisions-7.3.md` 里列的 serde 别名
//! （`policy` 的 `alias = "mode"`、`Mode` 的 `service/normal/off`）就可以安全删除。

use std::path::Path;

use serde_json::json;

use crate::report::Step;

/// 归一并落盘（`dry_run` 只读不写）。
pub fn normalize_step(path: &Path, dry_run: bool) -> Step {
    if !path.is_file() {
        return Step::skipped("service_toml", format!("{} 不存在", path.display()));
    }
    let normalized = match exvlan_control::legacy::service_toml::normalize_file(path) {
        Ok(value) => value,
        Err(err) => {
            return Step::skipped(
                "service_toml",
                format!("读取 {} 失败（需要管理员权限？）: {err}", path.display()),
            );
        }
    };
    if !normalized.changed {
        return Step::skipped("service_toml", "没有旧字段名/旧词汇").with_detail(
            json!({"path": path.display().to_string(), "runtime_alias_removal": ALIAS_SAFE}),
        );
    }
    let detail = json!({
        "path": path.display().to_string(),
        "renamed_mode": normalized.renames,
        "dropped_duplicate_mode": normalized.dropped_mode,
        "values_fixed": normalized.values_fixed,
        "unknown_values": normalized.unknown_values,
        // 结论进同一个步骤：下一轮删 `policy` 的 alias="mode" 与 Mode 旧词汇时有据可依。
        "runtime_alias_removal": ALIAS_SAFE,
    });
    if dry_run {
        return Step::planned("service_toml").with_detail(detail);
    }
    match write_atomically(path, &normalized.text) {
        Ok(()) => Step::ok("service_toml").with_detail(detail),
        Err(err) => Step::failed(
            "service_toml",
            format!(
                "写入 {} 失败: {err}（旧字段名保留，运行时别名仍需保留）",
                path.display()
            ),
        )
        .with_detail(detail),
    }
}

/// 临时文件 + rename；权限沿用 0600（服务配置由 root 持有）。
fn write_atomically(path: &Path, text: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("toml.new");
    std::fs::write(&tmp, text)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(err) => {
            // ignore-error: 临时文件清理失败不影响结论（下次覆盖同一路径）
            let _ = std::fs::remove_file(&tmp);
            Err(err)
        }
    }
}

/// 报告结论：`service.toml` 已归一 → 运行时的 `policy` 别名 / 旧词汇可以删。
pub const ALIAS_SAFE: &str =
    "safe: S8 已覆盖存量，可删除 `policy` 的 alias = \"mode\" 与 Mode 的 service/normal/off";
/// 报告结论：还有没归一的文件 → **不要**删别名。
pub const ALIAS_BLOCKED: &str = "blocked: 还有未归一的 service.toml，不要删除运行时别名";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::Status;
    use std::path::PathBuf;

    fn temp(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "exvlan-migrate-svc-{tag}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn normalizes_legacy_file_and_is_idempotent() {
        let dir = temp("normalize");
        let path = dir.join("service.toml");
        let original =
            "# 服务配置\nowner_uid = 501\nmode = \"normal\"\nrpc_mesh_whitelist = true\n";
        std::fs::write(&path, original).unwrap();

        let step = normalize_step(&path, false);
        assert_eq!(step.status, Status::Ok, "{step:?}");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "# 服务配置\nowner_uid = 501\npolicy = \"session\"\nrpc_mesh_whitelist = true\n"
        );

        // 幂等：第二次不改内容（也不改 mtime）
        let before = std::fs::metadata(&path).unwrap().modified().unwrap();
        let step = normalize_step(&path, false);
        assert_eq!(step.status, Status::Skipped);
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            before
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dry_run_does_not_write() {
        let dir = temp("dry");
        let path = dir.join("service.toml");
        std::fs::write(&path, "mode = \"off\"\n").unwrap();
        let step = normalize_step(&path, true);
        assert_eq!(step.status, Status::Planned);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "mode = \"off\"\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_file_is_skipped() {
        let dir = temp("missing");
        let step = normalize_step(&dir.join("service.toml"), false);
        assert_eq!(step.status, Status::Skipped);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn report_marks_alias_removal_safe_only_when_normalized() {
        let dir = temp("alias-note");
        let path = dir.join("service.toml");
        std::fs::write(&path, "mode = \"off\"\n").unwrap();
        let step = normalize_step(&path, false);
        assert_eq!(
            step.detail["runtime_alias_removal"].as_str(),
            Some(ALIAS_SAFE)
        );
        assert!(ALIAS_SAFE.contains("可删除"));
        assert!(ALIAS_BLOCKED.contains("不要"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
