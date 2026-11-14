//! `ui-state.json` → `prefs.toml`（S0）、旧 `[app].mode` 拆分（S1）、陈旧 `[misc]` 键清理（S2）。
//!
//! 为什么必须在这里做：`prefs.toml` 的唯一产出者原本是运行时
//! `packages/studio/src-tauri/src/prefs.rs`（`migrate_legacy` / `load_or_migrate`）。
//! 从 CLI 单独跑 `exvlan-migrate --yes`（或者下一版删掉运行时兼容之后）时，
//! 只删 `ui-state.json` 而不写 `prefs.toml` = 语言 / `log_level` /
//! `last_network_instance_id` / 诊断上传记录全部只剩备份里的一坨 JSON。
//!
//! 契约：
//! - 已存在的 `prefs.toml` **只补缺失键，绝不覆盖已有值**；
//! - 写失败 → `prefs_skipped`，**不删** `ui-state.json`，**不 bail**；
//! - 只有本步骤 `ok` 才允许删 `ui-state.json`；
//! - `ui-state.json.bak`（7.1 损坏快照）**只记录、不恢复**。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use toml::{Table, Value as TomlValue};

use crate::report::{Status, Step};

/// 陈旧 `[misc]` 键（前端 `localStorage` 写穿留下的界面缓存，不是设置）。
const STALE_MISC_KEYS: &[&str] = &["networkList", "upgrade_ignored_version"];

/// 一次 prefs 迁移的计划（**只读**算出结果，落盘由 [`PrefsPlan::apply`] 做）。
#[derive(Debug)]
pub struct PrefsPlan {
    pub path: PathBuf,
    /// 文件原本是否存在（决定备份 / 报告口径）。
    pub file_exists: bool,
    original: Table,
    updated: Table,
    /// 从 `ui-state.json` 新写入的键。
    imported: Vec<String>,
    /// `ui-state.json` 里有、但 `prefs.toml` 已有值 → 按契约不覆盖。
    kept: Vec<String>,
    /// 拆分的 `[app].mode` 结果。
    app_mode: AppModeStep,
    /// 删掉的陈旧 `[misc]` 键。
    stale_removed: Vec<String>,
    /// `ui-state.json` 存在但读不出来（损坏 / 权限）。
    ui_state_error: Option<String>,
    /// 只有 `.bak`（7.1 损坏快照）：只记录不恢复。
    bak_only: bool,
    /// `prefs.toml` 本身解析失败：不动它，也不删任何源。
    unparsable: Option<String>,
    /// `[app].mode` 解析失败的原始说明（保留原值，不 bail）。
    app_mode_invalid: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum AppModeStep {
    /// 没有 `[app].mode` 键。
    Absent,
    /// 解析成功并拆分（`target` / `remotes` 为合成结果）。
    Split {
        runtime: &'static str,
        target: Option<String>,
        remotes: usize,
        /// 瘦身后的 `[app].mode`（`None` = 键被删除）。
        kept: Option<String>,
    },
    Invalid(String),
}

impl PrefsPlan {
    /// 本次是否真的有改动（决定要不要备份 `prefs.toml`）。
    pub fn changed(&self) -> bool {
        self.unparsable.is_none() && self.updated != self.original
    }

    /// 是否可以安全删除 `ui-state.json`。
    fn safe_to_delete_ui_state(&self) -> bool {
        if self.unparsable.is_some() || self.ui_state_error.is_some() {
            return false;
        }
        if self.app_mode_invalid.is_some() {
            // app_mode 坏了不影响 prefs 的写入；但「保留原值」意味着这步没做完。
            return false;
        }
        true
    }

    /// 落盘（`dry_run` 时只算不写），返回 (prefs 步骤, 子步骤, 是否可删 ui-state)。
    pub fn apply(&self, dry_run: bool) -> (Step, Vec<Step>, bool) {
        let mut subs = Vec::new();
        subs.push(match &self.app_mode {
            AppModeStep::Absent => Step::skipped("app_mode", "prefs 里没有 [app].mode"),
            AppModeStep::Invalid(reason) => Step::skipped(
                "app_mode",
                format!("app_mode_invalid: {reason}（保留原值）"),
            ),
            AppModeStep::Split {
                runtime,
                target,
                remotes,
                kept,
            } => Step::new(
                "app_mode",
                if dry_run { Status::Planned } else { Status::Ok },
            )
            .with_detail(json!({
                "runtime": runtime,
                "target": target,
                "remotes": remotes,
                "kept_app_mode": kept.is_some(),
            })),
        });
        subs.push(if self.stale_removed.is_empty() {
            Step::skipped("prefs_misc", "没有陈旧键")
        } else {
            Step::new(
                "prefs_misc",
                if dry_run { Status::Planned } else { Status::Ok },
            )
            .with_detail(json!({"removed": self.stale_removed}))
        });

        let detail = json!({
            "path": self.path.display().to_string(),
            "imported": self.imported,
            "kept_existing": self.kept,
            "stale_removed": self.stale_removed,
            "ui_state_bak_only": self.bak_only,
            "changed": self.changed(),
        });

        if let Some(reason) = &self.unparsable {
            return (
                Step::skipped(
                    "prefs",
                    format!("prefs_unparsable: {reason}（保留 ui-state.json）"),
                )
                .with_detail(detail),
                subs,
                false,
            );
        }
        if let Some(reason) = &self.ui_state_error {
            subs.push(Step::skipped("ui_state_import", reason.clone()));
        }
        if !self.changed() {
            return (
                Step::skipped("prefs", "prefs.toml 已是最新").with_detail(detail),
                subs,
                self.safe_to_delete_ui_state(),
            );
        }
        if dry_run {
            return (Step::planned("prefs").with_detail(detail), subs, false);
        }
        match write_atomically(&self.path, &self.updated) {
            Ok(()) => (
                Step::ok("prefs").with_detail(detail),
                subs,
                self.safe_to_delete_ui_state(),
            ),
            Err(err) => {
                let reason = format!("prefs_skipped: 写入 {} 失败: {err}", self.path.display());
                (
                    Step::failed("prefs", reason).with_detail(detail),
                    subs,
                    false,
                )
            }
        }
    }
}

/// 计算计划（只读；`data` = 用户数据目录，`prefs.toml` / `ui-state.json` 都在这里）。
pub fn plan(data: &Path) -> PrefsPlan {
    let path = data.join("prefs.toml");
    let file_exists = path.is_file();
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => {
            return PrefsPlan {
                path,
                file_exists,
                original: Table::new(),
                updated: Table::new(),
                imported: Vec::new(),
                kept: Vec::new(),
                app_mode: AppModeStep::Absent,
                stale_removed: Vec::new(),
                ui_state_error: None,
                bak_only: false,
                unparsable: Some(format!("读取失败: {err}")),
                app_mode_invalid: None,
            };
        }
    };
    let parsed = if text.trim().is_empty() {
        Ok(Table::new())
    } else {
        toml::from_str::<Table>(&text)
    };
    let original = match parsed {
        Ok(table) => table,
        Err(err) => {
            return PrefsPlan {
                path,
                file_exists,
                original: Table::new(),
                updated: Table::new(),
                imported: Vec::new(),
                kept: Vec::new(),
                app_mode: AppModeStep::Absent,
                stale_removed: Vec::new(),
                ui_state_error: None,
                bak_only: false,
                unparsable: Some(format!("解析失败: {err}")),
                app_mode_invalid: None,
            };
        }
    };

    let mut plan = PrefsPlan {
        path,
        file_exists,
        original: original.clone(),
        updated: original,
        imported: Vec::new(),
        kept: Vec::new(),
        app_mode: AppModeStep::Absent,
        stale_removed: Vec::new(),
        ui_state_error: None,
        bak_only: false,
        unparsable: None,
        app_mode_invalid: None,
    };

    // 1) ui-state.json → prefs（只补缺失键）
    let ui_state = data.join("ui-state.json");
    let ui_bak = data.join("ui-state.json.bak");
    if ui_state.is_file() {
        match std::fs::read_to_string(&ui_state)
            .map_err(|err| format!("读取失败: {err}"))
            .and_then(|text| {
                serde_json::from_str::<BTreeMap<String, String>>(&text)
                    .map_err(|err| format!("不是 string→string 的 JSON 对象: {err}"))
            }) {
            Ok(map) => {
                for (key, value) in map {
                    import_key(&mut plan, &key, &value);
                }
            }
            Err(err) => {
                plan.ui_state_error = Some(format!("{}: {err}", ui_state.display()));
            }
        }
    } else if ui_bak.is_file() {
        // 7.1 把 ui-state.json 改名成 .bak；按约定只记录、不自动恢复。
        plan.bak_only = true;
    }

    // 2) [app].mode 拆分
    if let Some(raw) = plan
        .updated
        .get("app")
        .and_then(|app| app.get("mode"))
        .and_then(TomlValue::as_str)
        .map(str::to_string)
    {
        plan.split_app_mode(&raw);
    }

    // 3) 陈旧 [misc] 键
    if let Some(misc) = plan
        .updated
        .get_mut("misc")
        .and_then(TomlValue::as_table_mut)
    {
        for key in STALE_MISC_KEYS {
            if misc.remove(*key).is_some() {
                plan.stale_removed.push((*key).to_string());
            }
        }
    }

    plan
}

impl PrefsPlan {
    fn split_app_mode(&mut self, raw: &str) {
        use exvlan_control::legacy::app_mode::{self, AppModeOutcome};
        match app_mode::split(raw) {
            AppModeOutcome::Splittable(split) => {
                set_missing(&mut self.updated, "misc", "runtime", split.runtime);
                if let Some(target) = &split.target {
                    set_missing(&mut self.updated, "misc", "target", target);
                }
                if !split.remotes.is_empty() {
                    // 前端 `loadRemotes()` 按 `RemoteTarget[]` 解析，且 id 必须是
                    // `rm-legacy-<host>-<port>`（见 control::legacy::app_mode）。
                    let remotes =
                        serde_json::to_string(&split.remotes).unwrap_or_else(|_| "[]".into());
                    set_missing(&mut self.updated, "misc", "remotes", &remotes);
                }
                // 拆完只保留现役设置 `config_server_url`（前端从
                // `loadMode().config_server_url` 读），其余键已消费。
                match &split.keep {
                    Some(keep) => set_value(&mut self.updated, "app", "mode", keep),
                    None => {
                        if let Some(app) = self
                            .updated
                            .get_mut("app")
                            .and_then(TomlValue::as_table_mut)
                        {
                            app.remove("mode");
                        }
                    }
                }
                self.app_mode = AppModeStep::Split {
                    runtime: split.runtime,
                    target: split.target,
                    remotes: split.remotes.len(),
                    kept: split.keep,
                };
            }
            AppModeOutcome::Invalid(reason) => {
                // 保留原值：前端仍能按旧 JSON 工作，这里只记录不去动它。
                self.app_mode_invalid = Some(reason.clone());
                self.app_mode = AppModeStep::Invalid(reason);
            }
        }
    }
}

/// `ui-state.json` 的键 → `prefs.toml` 的段/键（与 `prefs.rs::set_key` 同一张表）。
fn import_key(plan: &mut PrefsPlan, key: &str, value: &str) {
    match key {
        "app_mode" => import(plan, "app", "mode", value),
        "lang" => import(plan, "app", "lang", value),
        "log_level" => import(plan, "app", "log_level", value),
        "last_network_instance_id" => import(plan, "ui", "last_network_instance_id", value),
        "diag_last_upload" => {
            // 旧值是 JSON `{at, urls}` → 拆成 `last_upload_url` + `updated_at`。
            let (url, at) = split_diag_upload(value);
            if let Some(url) = url {
                import(plan, "diag", "last_upload_url", &url);
            }
            if let Some(at) = at {
                import(plan, "diag", "updated_at", &at);
            }
        }
        other => import(plan, "misc", other, value),
    }
}

fn import(plan: &mut PrefsPlan, section: &str, key: &str, value: &str) {
    if get_str(&plan.updated, section, key).is_some() {
        plan.kept.push(format!("{section}.{key}"));
        return;
    }
    set_value(&mut plan.updated, section, key, value);
    plan.imported.push(format!("{section}.{key}"));
}

/// `{at,urls}` → `(urls, at)`；非 JSON 时原样当 URL。
fn split_diag_upload(raw: &str) -> (Option<String>, Option<String>) {
    let Ok(value) = serde_json::from_str::<Value>(raw) else {
        return (Some(raw.to_string()), None);
    };
    let urls = value
        .get("urls")
        .and_then(Value::as_str)
        .map(str::to_string);
    let at = value.get("at").and_then(Value::as_str).map(str::to_string);
    (urls, at)
}

fn get_str<'a>(table: &'a Table, section: &str, key: &str) -> Option<&'a str> {
    table.get(section)?.get(key)?.as_str()
}

fn set_missing(table: &mut Table, section: &str, key: &str, value: &str) {
    if get_str(table, section, key).is_some() {
        return;
    }
    set_value(table, section, key, value);
}

fn set_value(table: &mut Table, section: &str, key: &str, value: &str) {
    let entry = table
        .entry(section)
        .or_insert_with(|| TomlValue::Table(Table::new()));
    if let TomlValue::Table(section_table) = entry {
        section_table.insert(key.to_string(), TomlValue::String(value.to_string()));
    }
}

/// 临时文件 + rename + 读回校验（与 `prefs.rs::save` 同一套约定）。
fn write_atomically(path: &Path, table: &Table) -> std::io::Result<()> {
    let text = toml::to_string_pretty(table)
        .map_err(|err| std::io::Error::other(format!("序列化失败: {err}")))?;
    if text.trim().is_empty() {
        return Err(std::io::Error::other("序列化得到空内容，拒绝写入"));
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, &text)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&tmp, path)?;
    let back = std::fs::read_to_string(path).unwrap_or_default();
    if back != text {
        return Err(std::io::Error::other(format!(
            "写入校验失败（{}）：读回内容与写入不一致",
            path.display()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "exvlan-migrate-prefs-{tag}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn imports_ui_state_into_prefs() {
        let data = temp("import");
        std::fs::write(
            data.join("ui-state.json"),
            serde_json::to_string(&json!({
                "app_mode": "{\"mode\":\"service\"}",
                "lang": "en",
                "log_level": "debug",
                "last_network_instance_id": "inst-1",
                "diag_last_upload": "{\"at\":\"2026-01-02T03:04:05Z\",\"urls\":\"https://kv/1\"}",
                "some_ui_key": "v",
                "networkList": "[]",
            }))
            .unwrap(),
        )
        .unwrap();
        let plan = plan(&data);
        let (step, subs, safe) = plan.apply(false);
        assert_eq!(step.status, Status::Ok, "{step:?}");
        assert!(safe, "导入成功应允许删 ui-state.json");

        let text = std::fs::read_to_string(data.join("prefs.toml")).unwrap();
        let table: Table = toml::from_str(&text).unwrap();
        assert_eq!(get_str(&table, "app", "lang"), Some("en"));
        assert_eq!(get_str(&table, "app", "log_level"), Some("debug"));
        assert_eq!(
            get_str(&table, "ui", "last_network_instance_id"),
            Some("inst-1")
        );
        assert_eq!(
            get_str(&table, "diag", "last_upload_url"),
            Some("https://kv/1")
        );
        assert_eq!(
            get_str(&table, "diag", "updated_at"),
            Some("2026-01-02T03:04:05Z")
        );
        assert_eq!(get_str(&table, "misc", "some_ui_key"), Some("v"));
        // app_mode 被拆分：misc.runtime 落地、[app].mode 被瘦身/删除
        assert_eq!(get_str(&table, "misc", "runtime"), Some("service"));
        assert!(
            table.get("app").and_then(|a| a.get("mode")).is_none(),
            "mode=none 时整个 [app].mode 应删掉"
        );
        assert!(
            subs.iter()
                .any(|s| s.id == "app_mode" && s.status == Status::Ok)
        );
        // 陈旧键随导入一起被清掉
        assert!(get_str(&table, "misc", "networkList").is_none());
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn existing_prefs_only_gets_missing_keys() {
        let data = temp("merge");
        std::fs::write(
            data.join("prefs.toml"),
            "[app]\nlang = \"cn\"\n\n[misc]\nruntime = \"normal\"\n",
        )
        .unwrap();
        std::fs::write(
            data.join("ui-state.json"),
            serde_json::to_string(&json!({"lang": "en", "log_level": "warn"})).unwrap(),
        )
        .unwrap();
        let plan = plan(&data);
        plan.apply(false);
        let table: Table =
            toml::from_str(&std::fs::read_to_string(data.join("prefs.toml")).unwrap()).unwrap();
        // 已有值绝不覆盖
        assert_eq!(get_str(&table, "app", "lang"), Some("cn"));
        assert_eq!(get_str(&table, "misc", "runtime"), Some("normal"));
        // 缺的补上
        assert_eq!(get_str(&table, "app", "log_level"), Some("warn"));
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn existing_app_mode_never_overwrites_runtime() {
        let data = temp("app-mode-existing");
        std::fs::write(
            data.join("prefs.toml"),
            "[app]\nmode = '{\"mode\":\"remote\",\"remote_rpc_address\":\"10.1.2.3:2000\"}'\n\n[misc]\nruntime = \"normal\"\nremotes = \"[]\"\n",
        )
        .unwrap();
        let plan = plan(&data);
        plan.apply(false);
        let table: Table =
            toml::from_str(&std::fs::read_to_string(data.join("prefs.toml")).unwrap()).unwrap();
        assert_eq!(get_str(&table, "misc", "runtime"), Some("normal"));
        assert_eq!(get_str(&table, "misc", "remotes"), Some("[]"));
        // target 缺失 → 补上；[app].mode 已消费键被删掉
        assert_eq!(
            get_str(&table, "misc", "target"),
            Some("rm-legacy-10.1.2.3-2000")
        );
        assert!(table.get("app").and_then(|a| a.get("mode")).is_none());
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn write_failure_keeps_source_and_blocks_cleanup() {
        let data = temp("write-fail");
        std::fs::write(data.join("ui-state.json"), r#"{"lang":"en"}"#).unwrap();
        // 目标临时文件位置被目录占住 → 写入必失败（root 下 chmod 拦不住，用这个更可靠）
        std::fs::create_dir_all(data.join("prefs.toml.tmp")).unwrap();
        let plan = plan(&data);
        let (step, _, safe) = plan.apply(false);
        assert!(step.is_failure(), "{step:?}");
        assert!(
            step.reason
                .as_deref()
                .unwrap_or_default()
                .contains("prefs_skipped")
        );
        assert!(!safe, "写失败时不能删 ui-state.json");
        assert!(data.join("ui-state.json").is_file(), "源必须保留");
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn unparsable_prefs_is_left_alone() {
        let data = temp("unparsable");
        std::fs::write(data.join("prefs.toml"), "this is not toml [[[").unwrap();
        std::fs::write(data.join("ui-state.json"), r#"{"lang":"en"}"#).unwrap();
        let plan = plan(&data);
        let (step, _, safe) = plan.apply(false);
        assert_eq!(step.status, Status::Skipped);
        assert!(!safe);
        assert_eq!(
            std::fs::read_to_string(data.join("prefs.toml")).unwrap(),
            "this is not toml [[["
        );
        assert!(data.join("ui-state.json").is_file());
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn bak_only_is_recorded_not_restored() {
        let data = temp("bak-only");
        std::fs::write(data.join("ui-state.json.bak"), r#"{"lang":"en"}"#).unwrap();
        let plan = plan(&data);
        assert!(plan.bak_only);
        let (step, _, safe) = plan.apply(false);
        assert_eq!(step.status, Status::Skipped);
        assert!(safe, "没有 ui-state.json 时删不删都无源可丢");
        assert!(!data.join("prefs.toml").exists(), "bak 不能被恢复");
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn dry_run_does_not_write() {
        let data = temp("dry-run");
        std::fs::write(data.join("ui-state.json"), r#"{"lang":"en"}"#).unwrap();
        let plan = plan(&data);
        let (step, _, safe) = plan.apply(true);
        assert_eq!(step.status, Status::Planned);
        assert!(!safe, "dry-run 不允许任何清理");
        assert!(!data.join("prefs.toml").exists());
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn invalid_app_mode_keeps_raw_value() {
        let data = temp("invalid-app-mode");
        std::fs::write(data.join("prefs.toml"), "[app]\nmode = 'not-json'\n").unwrap();
        let plan = plan(&data);
        let (step, subs, safe) = plan.apply(false);
        let table: Table =
            toml::from_str(&std::fs::read_to_string(data.join("prefs.toml")).unwrap()).unwrap();
        assert_eq!(get_str(&table, "app", "mode"), Some("not-json"));
        assert!(!safe, "app_mode_invalid 时保留源（这一步没做完）");
        assert_eq!(step.status, Status::Skipped, "没改动就没写文件");
        assert!(subs.iter().any(|s| {
            s.id == "app_mode"
                && s.reason
                    .as_deref()
                    .unwrap_or_default()
                    .contains("app_mode_invalid")
        }));
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn diag_upload_without_json_stays_raw_url() {
        let (url, at) = split_diag_upload("https://kv/abc");
        assert_eq!(url.as_deref(), Some("https://kv/abc"));
        assert_eq!(at, None);
        let (url, at) = split_diag_upload(r#"{"at":"t","urls":"u"}"#);
        assert_eq!(url.as_deref(), Some("u"));
        assert_eq!(at.as_deref(), Some("t"));
    }
}
