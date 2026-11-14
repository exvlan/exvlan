//! 一次性迁移闸门（pre.17 专用）。
//!
//! 运行时**不做**迁移/兼容：检测到旧数据就阻塞启动，由用户确认后调用随包的
//! `exvlan-migrate`（它自己会备份/转换/清理/写标记）。下一版移除随包与自动执行。
//!
//! 检测口径**只有一份**：[`exvlan_control::legacy::detect`]。这里以前自己抄了一份
//! 文件清单，漏掉系统目录 `service.json` / `[app].mode` / 旧 machine_id / 旧服务定义 /
//! `stale_paths` —— 结果是「只有系统目录脏」的机器 `pending=false`，永不提示，
//! 而运行时也不再兼容读取 → 服务配置静默回落默认值。
//!
//! 工具跑不起来（安装包缺二进制 / 被杀软拦 / 迁移中途失败）时，闸门**不允许跳过**，
//! 所以这里必须同时给出可执行的下一步：[`MigrateGuide`] 把备份目录、日志、报告、
//! 手动命令一并交给前端展示，用户能自己排障后重试。

use serde::Serialize;
use std::path::{Path, PathBuf};
use tauri::Manager as _;

use super::MigrateGuide;

/// 备份目录前缀（与 `exvlan/migrate/src/main.rs` 的布局一致）。
const BACKUP_PREFIX: &str = "exvlan-migrate-backup-";
/// 备份目录里的报告文件（同 `exvlan/migrate/src/main.rs::finish_run`）。
const REPORT_NAME: &str = "migrate-report.json";
/// 工具默认日志文件（`exvlan/migrate/src/main.rs` 的 `--log` 默认值）。
const LOG_NAME: &str = "migrate.log";
/// 日志尾部上限：够看到最近一次失败的全部步骤，又不至于把 IPC 撑大。
const LOG_TAIL_BYTES: u64 = 16 * 1024;

#[derive(Debug, Clone, Serialize)]
pub struct MigrateStatus {
    /// 是否有旧数据需要迁移
    pub pending: bool,
    /// 检测到的遗留文件（给用户看清楚）
    pub items: Vec<String>,
    /// 随包迁移工具路径（None = 安装包不完整）
    pub tool: Option<String>,
    /// 是否已完成过迁移
    pub done: bool,
    /// 失败指引（路径/命令/日志尾部），全部由后端计算
    pub guide: Option<MigrateGuide>,
}

/// 旧版本遗留数据：与 `exvlan-migrate` 共用同一份检测（`exvlan_control::legacy`）。
pub fn legacy_items() -> Vec<String> {
    let user = exvlan_control::paths::user_paths();
    let p = exvlan_control::paths::paths();
    exvlan_control::legacy::detect(&user, &p).item_labels()
}

/// 随包迁移工具的所有可能位置（Tauri 会保留 resources 里的相对路径，几种形态都试）。
fn tool_candidates(app: &tauri::AppHandle) -> Vec<PathBuf> {
    let Ok(dir) = app.path().resource_dir() else {
        return Vec::new();
    };
    let name = if cfg!(windows) {
        "exvlan-migrate.exe"
    } else {
        "exvlan-migrate"
    };
    vec![
        dir.join("tools").join(name),
        dir.join("resources").join("tools").join(name),
        dir.join(name),
    ]
}

/// 随包迁移工具（存在才算）。
fn tool_path(app: &tauri::AppHandle) -> Option<PathBuf> {
    tool_candidates(app).into_iter().find(|p| p.is_file())
}

/// 工具「应该」在的目录：即使二进制缺失也要给，用户才知道去哪找/放。
fn tool_dir(app: &tauri::AppHandle) -> Option<PathBuf> {
    tool_candidates(app)
        .first()
        .and_then(|p| p.parent())
        .map(Path::to_path_buf)
}

/// 最近一次备份目录（`<数据目录>/exvlan-migrate-backup-*` 里最新的一个）。
fn latest_backup_dir(data: &Path) -> Option<PathBuf> {
    std::fs::read_dir(data)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_dir()
                && path
                    .file_name()
                    .map(|name| name.to_string_lossy().starts_with(BACKUP_PREFIX))
                    .unwrap_or(false)
        })
        .filter_map(|path| {
            let mtime = std::fs::metadata(&path).ok()?.modified().ok()?;
            Some((mtime, path))
        })
        .max_by_key(|(mtime, _)| *mtime)
        .map(|(_, path)| path)
}

/// 读文件尾部（最多 `max_bytes`）。文件不存在/读不动时返回空串：诊断信息不该反过来阻断启动。
fn tail_text(path: &Path, max_bytes: u64) -> String {
    use std::io::{Read as _, Seek as _, SeekFrom};
    let Ok(mut file) = std::fs::File::open(path) else {
        return String::new();
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    if len > max_bytes {
        let _ = file.seek(SeekFrom::End(-(max_bytes as i64)));
    }
    let mut buf = Vec::new();
    let _ = file.take(max_bytes).read_to_end(&mut buf);
    String::from_utf8_lossy(&buf).into_owned()
}

/// 可直接粘贴执行的命令：与 GUI 实际调用参数一致（提权 + 备份属主交回桌面用户）。
fn cli_command(tool: &Path, uid: u32) -> String {
    let exe = format!("\"{}\"", tool.display());
    if cfg!(windows) {
        // Windows 没有 sudo：提示在「以管理员身份运行」的终端里执行。
        format!("{exe} --yes")
    } else {
        format!("sudo {exe} --yes --owner-uid {uid}")
    }
}

/// 失败指引：所有路径/命令在**这里**拼好，前端只展示与复制。
fn guide(app: &tauri::AppHandle) -> MigrateGuide {
    let user = exvlan_control::paths::user_paths();
    let log_file = user.logs.join(LOG_NAME);
    let backup = latest_backup_dir(&user.data);
    let report = backup
        .as_ref()
        .map(|dir| dir.join(REPORT_NAME))
        .filter(|path| path.is_file());
    let tool = tool_path(app);
    MigrateGuide {
        data_dir: user.data.display().to_string(),
        log_file: log_file.display().to_string(),
        backup_dir: backup.map(|p| p.display().to_string()),
        report_file: report.map(|p| p.display().to_string()),
        tool_dir: tool_dir(app).map(|p| p.display().to_string()),
        command: tool.map(|p| cli_command(&p, crate::service_client::current_uid())),
        log_tail: tail_text(&log_file, LOG_TAIL_BYTES),
    }
}

#[tauri::command]
pub fn migrate_status(app: tauri::AppHandle) -> MigrateStatus {
    let user = exvlan_control::paths::user_paths();
    // 标记版本升 v3：只跑过旧版 `migrate.done` 的机器视为「还没跑过」，
    // 于是会再跑一次把 `service.toml` 的旧字段名归一掉（各步幂等，已归一的
    // 机器零改动）。见 `exvlan_control::legacy::MIGRATE_DONE_MARKER`。
    let done = user
        .data
        .join(exvlan_control::legacy::MIGRATE_DONE_MARKER)
        .is_file();
    let items = legacy_items();
    let pending = !done && !items.is_empty();
    MigrateStatus {
        // 没有遗留项时不该弹（`done` 只影响列表里的「已完成」标记）。
        pending,
        items,
        tool: tool_path(&app).map(|p| p.display().to_string()),
        done,
        // 不 pending 时前端不显示闸门，也就用不到指引（省一次目录扫描）。
        guide: pending.then(|| guide(&app)),
    }
}

/// 工具缺失时的用户文案（提权/被拦都走这条路，必须给出下一步而不是一句「失败」）。
fn missing_tool_message(dir: Option<&Path>) -> String {
    let dir = dir
        .map(|d| d.display().to_string())
        .unwrap_or_else(|| "tools".to_string());
    format!(
        "安装包里没有 exvlan-migrate（安装包不完整，或被安全软件拦截）：{dir}。\
         请在该目录下运行 exvlan-migrate --yes，或重新下载安装包后点「重新检测」"
    )
}

/// 只保留 stderr 最后 `lines` 行非空内容：原样给用户看「卡在哪一步」，
/// 又不至于把整个运行日志塞爆界面。
fn stderr_tail(stderr: &str, lines: usize) -> String {
    stderr
        .lines()
        .filter(|line| !line.trim().is_empty())
        .rev()
        .take(lines)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n")
}

/// 执行随包迁移工具（提权一次），返回它打印的 JSON 报告。
#[tauri::command]
pub async fn migrate_run(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    let result = run(&app).await;
    // 成功/失败都要落 error.log：「复制/上传诊断」只读这一份，
    // 不记下来现场就只剩用户嘴上那句「迁移失败」。
    match &result {
        Ok(report) => crate::logger::record("migrate", format!("done: {report}")),
        Err(err) => crate::logger::record("migrate", format!("failed: {err}")),
    }
    result
}

async fn run(app: &tauri::AppHandle) -> Result<serde_json::Value, String> {
    let bin = tool_path(app).ok_or_else(|| missing_tool_message(tool_dir(app).as_deref()))?;
    // 失败文案带日志路径：用户不必再猜 migrate.log 在哪。
    let log_file = exvlan_control::paths::user_paths().logs.join(LOG_NAME);
    let log_file = log_file.display().to_string();
    let log_hint = log_file.clone();
    let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
        let mut cmd = std::process::Command::new(&bin);
        // 提权运行 → 备份/marker 会变成 root 所有，用户读不了自己的备份；
        // 把当前桌面用户 uid 传进去，由工具 chown 回来。
        cmd.args([
            "--yes",
            "--json",
            "--owner-uid",
            &crate::service_client::current_uid().to_string(),
        ]);
        let out = crate::elevate::Command::new(cmd)
            .output()
            .map_err(|e| format!("调用迁移工具失败: {e}\n日志：{log_hint}"))?;
        let stdout = String::from_utf8_lossy(&out.stdout);
        // 工具会写日志到 stderr；把最后一行 JSON 解析出来（前面可能有日志）。
        let json_line = stdout
            .lines()
            .rev()
            .find(|l| l.trim_start().starts_with('{'))
            .unwrap_or("");
        let value = if json_line.is_empty() {
            serde_json::json!({"ok": out.status.success()})
        } else {
            serde_json::from_str(json_line)
                .unwrap_or_else(|_| serde_json::json!({"ok": out.status.success()}))
        };
        if !out.status.success() {
            // 原样给最后几行 stderr（一步一条，能直接看出卡在哪），别只留最后一行。
            let tail = stderr_tail(&String::from_utf8_lossy(&out.stderr), 20);
            return Err(format!(
                "迁移失败（退出码 {:?}）：\n{}\n日志：{log_hint}",
                out.status.code(),
                tail
            ));
        }
        Ok(value)
    })
    .await
    .map_err(|e| format!("迁移任务失败: {e}\n日志：{log_file}"))??;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "exvlan-studio-migrate-test-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    /// 失败指引必须指到**真实存在**的备份目录，而不是拼一个不存在的路径。
    #[test]
    fn latest_backup_dir_picks_newest_existing_dir() {
        let root = temp("backup-pick");
        std::fs::create_dir_all(root.join("exvlan-migrate-backup-100")).unwrap();
        // 两次创建隔开一点，避免文件系统时间戳精度不足时并列。
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::create_dir_all(root.join("exvlan-migrate-backup-200")).unwrap();
        std::fs::create_dir_all(root.join("not-a-backup")).unwrap();
        let newest = root.join("exvlan-migrate-backup-200").join(REPORT_NAME);
        std::fs::write(&newest, "{}").unwrap();
        let picked = latest_backup_dir(&root).expect("应挑出备份目录");
        assert!(picked.ends_with("exvlan-migrate-backup-200"), "{picked:?}");
        assert!(picked.join(REPORT_NAME).is_file());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn latest_backup_dir_is_none_without_backups() {
        let root = temp("backup-none");
        std::fs::write(root.join("prefs.toml"), "").unwrap();
        assert!(latest_backup_dir(&root).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 日志尾部：超大文件只读最后一段，且边界上的多字节字符不会 panic。
    #[test]
    fn tail_text_reads_only_the_end() {
        let root = temp("tail");
        let log = root.join(LOG_NAME);
        let body = format!("{}{}", "老日志\n".repeat(5000), "最后一行失败原因\n");
        std::fs::write(&log, &body).unwrap();
        let tail = tail_text(&log, 64);
        // 截断点可能落在多字节字符中间（会被替换成 U+FFFD），所以按字节数上界判断。
        assert!(tail.chars().count() <= 64, "{}", tail.chars().count());
        assert!(tail.len() < body.len());
        assert!(tail.contains("最后一行失败原因"), "{tail}");
        assert!(tail_text(&root.join("missing.log"), 64).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 复制给用户的命令必须带绝对路径，并和 GUI 实际调用一致（unix 加 sudo + owner-uid）。
    #[test]
    fn cli_command_is_absolute_and_matches_gui_args() {
        let tool = std::env::temp_dir()
            .join("exvlan-migrate-bin")
            .join("exvlan-migrate");
        let cmd = cli_command(&tool, 1000);
        assert!(cmd.contains(&tool.display().to_string()), "{cmd}");
        assert!(cmd.contains("--yes"));
        if cfg!(windows) {
            assert!(!cmd.contains("--owner-uid"), "{cmd}");
        } else {
            assert!(cmd.contains("sudo "), "{cmd}");
            assert!(cmd.contains("--owner-uid 1000"), "{cmd}");
        }
    }

    /// 报告文件只在真的存在时才给出（别让用户去点一个不存在的路径）。
    #[test]
    fn report_file_is_only_reported_when_present() {
        let root = temp("report");
        std::fs::create_dir_all(root.join("exvlan-migrate-backup-9")).unwrap();
        let backup = latest_backup_dir(&root).unwrap();
        let report = backup.join(REPORT_NAME);
        assert!(!report.is_file());
        std::fs::write(&report, "{}").unwrap();
        assert!(
            backup.join(REPORT_NAME).is_file(),
            "收尾写出报告后必须可检测"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **失败态模拟**（安装包里没有工具）：文案必须给出工具应在的目录与下一步，
    /// 不能只说「失败」。这是「不许跳过」，所以这里必须是可执行的指引。
    #[test]
    fn missing_tool_message_points_at_tool_dir_and_next_step() {
        let dir = temp("no-tool");
        let msg = missing_tool_message(Some(&dir));
        assert!(msg.contains(&dir.display().to_string()), "{msg}");
        assert!(msg.contains("exvlan-migrate --yes"), "{msg}");
        assert!(msg.contains("重新检测"), "{msg}");
        // resource_dir 拿不到时也不能 panic / 出现空目录名。
        let fallback = missing_tool_message(None);
        assert!(fallback.contains("tools"), "{fallback}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **失败态模拟**（工具跑起来但退出码非 0）：多行 stderr 要原样保留最后几行，
    /// 而不是只剩最后一行（只留一行时用户看不出卡在哪一步）。
    #[test]
    fn stderr_tail_keeps_last_lines_verbatim() {
        let stderr = (1..=30)
            .map(|i| format!("unix:{i} [migrate] ERROR step-{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let tail = stderr_tail(&stderr, 20);
        let lines: Vec<&str> = tail.lines().collect();
        assert_eq!(lines.len(), 20);
        assert!(lines[0].contains("step-11"), "{tail}");
        assert!(lines[19].contains("step-30"), "{tail}");
        // 空行不占名额
        assert_eq!(stderr_tail("a\n\n\nb\n", 5), "a\nb");
        assert_eq!(stderr_tail("", 5), "");
    }
}
