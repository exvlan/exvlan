//! `machine_id` 归位（S3）。
//!
//! **高风险**：删错 / 写错都会让老用户变成「新设备」—— peer_id 变化 →
//! 设备登记、已配对 token、远端 gateway 的授权全部对不上。
//!
//! 规则：
//! - 新路径 `<machine_id_state_dir>/machine_id` 已存在 → **一律不动**；
//! - 旧路径按优先级 COPY：[`exvlan_control::paths::legacy_machine_id_sources`]
//!   （`<data>/exvlan/machine_id` → `<core>/et_machine_id`）；
//! - **不删旧文件**（下次幂等再判，回滚也不依赖它）；
//! - 读不到 / 不是合法 UUID → 告警跳过，**不 bail**。
//!
//! 放弃的一支：旧 `machine_uid` 哈希（`machine_id.rs::resolve_legacy_machine_uid_hash`）。
//! 它需要 `machine_uid` crate + 内核 `digest_uuid_from_str`（std `DefaultHasher`）
//! 逐位一致，且 Linux 上还掺了 hostname/MAC；而这一支只在「状态目录非空、但
//! machine_id 从未落盘」时生效 —— 那种机器旧版本每次启动也是现算，7.3 运行时同样
//! 现算（`resolve_new_machine_id`），没有「已存在的身份」可保留。强行复算反而可能
//! 写出一个与旧 peer_id 不同的值。详见 `docs/migrate-7.3.md`。

use std::path::Path;

use serde_json::json;

use crate::report::Step;

/// 一次 machine_id 迁移的计划（只读）。
#[derive(Debug, Clone)]
pub struct MachineIdPlan {
    pub from: std::path::PathBuf,
    pub to: std::path::PathBuf,
}

impl MachineIdPlan {
    pub fn apply(&self, dry_run: bool) -> Step {
        let detail = json!({
            "from": self.from.display().to_string(),
            "to": self.to.display().to_string(),
        });
        if dry_run {
            return Step::planned("machine_id").with_detail(detail);
        }
        match copy_machine_id(&self.from, &self.to) {
            Ok(()) => Step::ok("machine_id").with_detail(detail),
            Err(err) => Step::failed(
                "machine_id",
                format!(
                    "复制 {} → {} 失败: {err}（旧文件保留，未改动新路径）",
                    self.from.display(),
                    self.to.display()
                ),
            ),
        }
    }
}

/// 选出第一个**合法**的旧落点（优先级由调用方给的顺序决定）。
pub fn select_source(
    candidates: &[std::path::PathBuf],
    to: &Path,
) -> Result<Option<std::path::PathBuf>, String> {
    let mut problems = Vec::new();
    for path in candidates {
        if path == to || !path.is_file() {
            continue;
        }
        match std::fs::read_to_string(path) {
            Ok(text) => match text.trim().parse::<uuid::Uuid>() {
                Ok(_) => return Ok(Some(path.clone())),
                Err(err) => problems.push(format!("{}: 不是合法 UUID（{err}）", path.display())),
            },
            Err(err) => problems.push(format!("{}: 读不到（{err}）", path.display())),
        }
    }
    if problems.is_empty() {
        Ok(None)
    } else {
        Err(problems.join("; "))
    }
}

/// COPY（源保留）。目录不存在时创建；Unix 上收紧到 0600。
fn copy_machine_id(from: &Path, to: &Path) -> std::io::Result<()> {
    if to.exists() {
        return Ok(());
    }
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::copy(from, to)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(to, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// 计算计划：新路径已存在 → `None`（不动）；否则从旧落点里挑第一个合法的。
pub fn plan(core: &Path) -> (Option<MachineIdPlan>, Option<Step>) {
    let Some(dir) = exvlan_control::paths::machine_id_state_dir() else {
        return (
            None,
            Some(Step::skipped(
                "machine_id",
                "当前平台无法定位 machine_id 目录",
            )),
        );
    };
    let to = dir.join("machine_id");
    if to.exists() {
        return (
            None,
            Some(
                Step::skipped(
                    "machine_id",
                    format!("新路径已存在，不动: {}", to.display()),
                )
                .with_detail(json!({"to": to.display().to_string()})),
            ),
        );
    }
    let candidates = exvlan_control::paths::legacy_machine_id_sources(core);
    match select_source(&candidates, &to) {
        Ok(Some(from)) => (Some(MachineIdPlan { from, to }), None),
        Ok(None) => (
            None,
            Some(Step::skipped(
                "machine_id",
                "没有旧 machine_id 可复制（新装机器不需要）",
            )),
        ),
        Err(problems) => (
            None,
            Some(
                Step::skipped("machine_id", format!("旧 machine_id 不可用: {problems}"))
                    .with_detail(json!({"candidates": candidates
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()})),
            ),
        ),
    }
}

/// report 里必须明确写出「放弃 uid 哈希」这一支。
pub fn uid_hash_note() -> Step {
    Step::skipped(
        "machine_id_uid_hash",
        "放弃：需要 machine_uid + 内核 digest 逐位复算；该分支只在「状态目录非空但从未落盘」时生效，无已存在身份可保留（见 docs/migrate-7.3.md）",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::Status;
    use std::path::PathBuf;

    fn temp(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "exvlan-migrate-mid-{tag}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    const ID: &str = "11111111-2222-3333-4444-555555555555";

    #[test]
    fn copies_legacy_and_keeps_source() {
        let dir = temp("copy");
        let from = dir.join("exvlan").join("machine_id");
        std::fs::create_dir_all(from.parent().unwrap()).unwrap();
        std::fs::write(&from, ID).unwrap();
        let to = dir.join("era.exvlan.studio").join("machine_id");

        let plan = MachineIdPlan {
            from: from.clone(),
            to: to.clone(),
        };
        let step = plan.apply(false);
        assert_eq!(step.status, Status::Ok, "{step:?}");
        assert_eq!(std::fs::read_to_string(&to).unwrap(), ID);
        assert!(from.is_file(), "旧文件必须保留（幂等再判）");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn never_overwrites_existing_new_path() {
        let dir = temp("existing");
        let to = dir.join("machine_id");
        std::fs::write(&to, "99999999-9999-9999-9999-999999999999").unwrap();
        let plan = MachineIdPlan {
            from: dir.join("old").join("machine_id"),
            to: to.clone(),
        };
        let step = plan.apply(false);
        assert_eq!(step.status, Status::Ok);
        // 新路径内容不变
        assert_eq!(
            std::fs::read_to_string(&to).unwrap(),
            "99999999-9999-9999-9999-999999999999"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn picks_first_valid_candidate_and_reports_garbage() {
        let dir = temp("select");
        let bad = dir.join("bad").join("machine_id");
        let good = dir.join("good").join("machine_id");
        std::fs::create_dir_all(bad.parent().unwrap()).unwrap();
        std::fs::create_dir_all(good.parent().unwrap()).unwrap();
        std::fs::write(&bad, "not-a-uuid").unwrap();
        std::fs::write(&good, ID).unwrap();
        let to = dir.join("new").join("machine_id");
        let picked = select_source(&[bad.clone(), good.clone()], &to).unwrap();
        assert_eq!(picked, Some(good));
        // 全都非法 → Err（告警跳过，不 bail）
        let err = select_source(&[bad], &to).unwrap_err();
        assert!(err.contains("不是合法 UUID"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dry_run_reports_but_does_not_copy() {
        let dir = temp("dry");
        let from = dir.join("exvlan").join("machine_id");
        std::fs::create_dir_all(from.parent().unwrap()).unwrap();
        std::fs::write(&from, ID).unwrap();
        let to = dir.join("new").join("machine_id");
        let step = MachineIdPlan {
            from,
            to: to.clone(),
        }
        .apply(true);
        assert_eq!(step.status, Status::Planned);
        assert!(!to.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
