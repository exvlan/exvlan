//! GUI 侧统一错误出口：所有异常写入 `<日志目录>/error.log`。
//!
//! 为什么单独一个文件：`exvlan.log` 是全量滚动日志（内核 + GUI），出问题时
//! 动辄数十 MB，用户很难往外发。`error.log` 只收「错误 + 关键节点」，
//! 体积小、可直接分享，用来定位用户机器上的异常。
//!
//! 约定：
//! - 单行一条，格式 `RFC3339 [scope] message`，便于 grep；
//! - 5 MB × 3 份轮转（`error.log` / `.1` / `.2`）；
//! - 默认脱敏（`network_secret` / 私钥 / 密码 / token）；
//! - 任何写入失败都静默，绝不因为日志拖垮业务；
//! - 初始化前发生的错误先放内存环形缓冲，初始化后回放。

use std::fmt::Display;
use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// 磁盘文件名。
pub const FILE_NAME: &str = "error.log";
const MAX_BYTES: u64 = 16 * 1024 * 1024;
const KEEP_FILES: usize = 3;
const BUFFER_LIMIT: usize = 200;
const MAX_MESSAGE_BYTES: usize = 8 * 1024;

struct State {
    dir: Option<PathBuf>,
    file: Option<File>,
    size: u64,
    pending: Vec<String>,
    /// 上次尝试重新打开文件的时间（避免每写一行都去 open 一次）。
    last_reopen: Option<std::time::Instant>,
}

fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(|| {
        Mutex::new(State {
            dir: None,
            file: None,
            size: 0,
            pending: Vec::new(),
            last_reopen: None,
        })
    })
}

/// 取状态锁；即使锁被毒化也继续使用内部状态。
///
/// 一次 panic 不能让日志通道永久静默 —— 那会让「用户报错时我们什么都看不到」
/// 这种最难排查的情况反复发生。
fn lock_state() -> std::sync::MutexGuard<'static, State> {
    match state().lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// 按顺序尝试多个候选目录，把 `error.log` 落到**第一个可写**的位置。
///
/// 背景：20709 在 macOS 上踩到 `~/Library/Logs/<id>` 被 root 占用（守护进程写过），
/// 用户态无法创建文件 → 「error.log 尚未初始化」。这里必须有多级兜底，
/// 并把每次尝试的结果记录下来，否则连「为什么没日志」都查不到。
pub fn init_best(candidates: &[PathBuf]) -> Option<PathBuf> {
    let mut attempts: Vec<String> = Vec::new();
    for dir in candidates {
        match open_in(dir) {
            Ok((file, size)) => {
                let mut st = lock_state();
                st.dir = Some(dir.clone());
                st.file = Some(file);
                st.size = size;
                let pending = std::mem::take(&mut st.pending);
                for line in pending {
                    st.write_line(&line);
                }
                drop(st);
                let _ = PANIC_PATH.set(dir.join(FILE_NAME));
                if !attempts.is_empty() {
                    record("error_log.fallback", attempts.join(" ; "));
                }
                return Some(dir.join(FILE_NAME));
            }
            Err(err) => attempts.push(format!("{}: {err}", dir.display())),
        }
    }
    eprintln!("[error_log] 所有候选目录都不可写: {}", attempts.join(" ; "));
    None
}

fn open_in(dir: &Path) -> std::io::Result<(File, u64)> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(FILE_NAME);
    let file = OpenOptions::new().create(true).append(true).open(&path)?;
    let size = file.metadata().map(|m| m.len()).unwrap_or_default();
    Ok((file, size))
}

/// panic 专用的落盘路径（初始化成功后固定）。
///
/// panic hook 里**不能**再走 `record()`：如果 panic 恰好发生在持有状态锁的时候，
/// 会死锁，应用表现为卡死而不是崩溃。这里只做一次无锁的文件追加。
static PANIC_PATH: OnceLock<PathBuf> = OnceLock::new();

/// panic hook 专用：无锁追加一行 `[panic]`。
pub fn record_panic(message: impl Display) {
    let text = redact(&message.to_string())
        .replace('\n', " | ")
        .replace('\r', "");
    let line = format!("{} [panic] {text}\n", timestamp());
    match PANIC_PATH.get() {
        Some(path) => {
            if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
                let _ = f.write_all(line.as_bytes());
            }
        }
        None => eprintln!("{}", line.trim_end()),
    }
}

/// 当前是否已初始化。
pub fn is_initialized() -> bool {
    lock_state().file.is_some()
}

/// 当前日志文件路径（未初始化时为 `None`）。
pub fn path() -> Option<PathBuf> {
    lock_state().dir.clone().map(|d| d.join(FILE_NAME))
}

/// 读取最近 `max_bytes` 字节（给「复制/分享日志」用）。
pub fn tail(max_bytes: usize) -> String {
    let Some(path) = path() else {
        return String::new();
    };
    let Ok(bytes) = std::fs::read(&path) else {
        return String::new();
    };
    let start = bytes.len().saturating_sub(max_bytes);
    String::from_utf8_lossy(&bytes[start..]).into_owned()
}

/// 记录一条错误 / 关键节点。
pub fn record(scope: &str, message: impl Display) {
    let text = truncate(&redact(&message.to_string()), MAX_MESSAGE_BYTES);
    // 多行消息（anyhow 链、launchctl 输出）压成单行，避免破坏 grep 语义。
    let text = text.replace('\n', " | ").replace('\r', "");
    let line = format!("{} [{scope}] {text}\n", timestamp());

    let mut st = lock_state();
    // 句柄丢失（轮转失败/文件被删/权限变化）时按 30s 节流重试重新打开。
    if st.file.is_none() && st.dir.is_some() {
        st.try_reopen();
    }
    if st.file.is_some() {
        st.write_line(&line);
        return;
    }

    let uninitialized = st.dir.is_none();
    if st.pending.len() >= BUFFER_LIMIT {
        st.pending.remove(0);
    }
    st.pending.push(line.clone());
    drop(st);

    // 已初始化却写不进去：至少让事件日志留痕（它用的是另一把锁）。
    if !uninitialized {
        event("log-fallback", line.trim_end());
    }
}

impl State {
    /// 重新打开日志文件（30s 节流）。
    fn try_reopen(&mut self) {
        let Some(dir) = self.dir.clone() else {
            return;
        };
        let now = std::time::Instant::now();
        if let Some(last) = self.last_reopen
            && now.duration_since(last) < std::time::Duration::from_secs(30)
        {
            return;
        }
        self.last_reopen = Some(now);
        match open_in(&dir) {
            Ok((file, size)) => {
                self.file = Some(file);
                self.size = size;
            }
            Err(err) => eprintln!("[error_log] 重新打开 {} 失败: {err}", dir.display()),
        }
    }

    fn write_line(&mut self, line: &str) {
        // 轮转：写之前先判断，保证单文件不超过上限
        let line_len = line.len() as u64;
        if self.size + line_len > MAX_BYTES {
            self.rotate();
        }
        let Some(file) = self.file.as_mut() else {
            return;
        };
        if file.write_all(line.as_bytes()).is_ok() {
            self.size += line_len;
        }
    }

    fn rotate(&mut self) {
        let Some(dir) = self.dir.clone() else {
            return;
        };
        self.file = None;
        let base = dir.join(FILE_NAME);
        // error.log.(KEEP-1) 丢弃，其余依次后移一位，最后把 error.log 变成 .1
        let oldest = dir.join(format!("{FILE_NAME}.{}", KEEP_FILES - 1));
        let _ = std::fs::remove_file(&oldest);
        for idx in (1..KEEP_FILES - 1).rev() {
            let from = dir.join(format!("{FILE_NAME}.{idx}"));
            let to = dir.join(format!("{FILE_NAME}.{}", idx + 1));
            let _ = std::fs::rename(&from, &to);
        }
        let _ = std::fs::rename(&base, dir.join(format!("{FILE_NAME}.1")));

        match OpenOptions::new().create(true).append(true).open(&base) {
            Ok(f) => {
                self.file = Some(f);
                self.size = 0;
            }
            Err(err) => eprintln!("[error_log] 轮转后重新打开 {} 失败: {err}", base.display()),
        }
    }
}

/// 命令层统一错误转换：记录后返回 `String`（与原 `e.to_string()` 行为一致）。
///
/// `anyhow::Error` 会自动展开完整因果链（`{:#}`），其他类型用 `Display`。
pub fn other<E: Display + 'static>(scope: &'static str, e: E) -> String {
    match (&e as &dyn std::any::Any).downcast_ref::<anyhow::Error>() {
        Some(chain) => record(scope, format!("{chain:#}")),
        None => record(scope, &e),
    }
    e.to_string()
}

/// 脱敏：把敏感字段的值替换成 `***`。
///
/// 覆盖 JSON（`"key": "value"`）与 key=value 两种形态。
pub fn redact(input: &str) -> String {
    const KEYS: &[&str] = &[
        "network_secret",
        "wireguard_private_key",
        "credential_secret",
        "local_private_key",
        "private_key",
        "password",
        "secret",
        "token",
        "authorization",
    ];

    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if let Some(key) = KEYS.iter().find(|k| matches_key_at(input, i, k)) {
            let key_end = i + key.len();
            out.push_str(&input[i..key_end]);
            // 找到分隔符
            let mut j = key_end;
            while j < bytes.len() && !matches!(bytes[j], b':' | b'=') {
                out.push(bytes[j] as char);
                j += 1;
            }
            if j >= bytes.len() {
                i = j;
                continue;
            }
            out.push(bytes[j] as char);
            j += 1;
            // 跳过空白
            while j < bytes.len() && matches!(bytes[j], b' ' | b'\t') {
                out.push(bytes[j] as char);
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'"' {
                out.push('"');
                j += 1;
                while j < bytes.len() && bytes[j] != b'"' {
                    j += 1;
                }
                out.push('*');
                out.push('*');
                out.push('*');
                if j < bytes.len() {
                    out.push('"');
                    j += 1;
                }
            } else {
                while j < bytes.len()
                    && !matches!(bytes[j], b',' | b'}' | b'\n' | b'\r' | b' ' | b'\t')
                {
                    j += 1;
                }
                out.push_str("***");
            }
            i = j;
            continue;
        }
        let ch = input[i..].chars().next().unwrap_or('?');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn matches_key_at(input: &str, idx: usize, key: &str) -> bool {
    let rest = &input[idx..];
    if !rest.starts_with(key) {
        return false;
    }
    // 前后必须是「非标识符字符」，避免 password_hash 之类误判
    let before_ok = idx == 0 || {
        let prev = input[..idx].chars().next_back().unwrap_or(' ');
        !prev.is_alphanumeric() && prev != '_'
    };
    let after = rest[key.len()..].chars().next();
    let after_ok = match after {
        None => true,
        Some(c) => !c.is_alphanumeric() && c != '_',
    };
    before_ok && after_ok
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…(truncated {} bytes)", &s[..end], s.len())
}

/// `2026-11-03T12:34:56.789Z`
pub(crate) fn timestamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs() as i64;
    let millis = now.subsec_millis();
    let days = secs.div_euclid(86_400);
    let sod = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        sod / 3600,
        (sod % 3600) / 60,
        sod % 60
    )
}

/// Howard Hinnant 的 civil_from_days（UTC，无闰秒）。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

// ---------------------------------------------------------------------------
// events.log：GUI 侧事件流（FFI 调用 / VPN 权限 / TUN / 实例启停）
//
// 内核自身的 tracing 不一定会落到文件（Android 上尤其），事件流由 GUI 自己记，
// 保证组网问题至少有调用链可看。
// ---------------------------------------------------------------------------

pub const EVENTS_FILE: &str = "events.log";
const EVENTS_MAX_BYTES: u64 = 10 * 1024 * 1024;

struct EventsState {
    dir: Option<PathBuf>,
    file: Option<File>,
    size: u64,
}

fn events_state() -> &'static Mutex<EventsState> {
    static STATE: OnceLock<Mutex<EventsState>> = OnceLock::new();
    STATE.get_or_init(|| {
        Mutex::new(EventsState {
            dir: None,
            file: None,
            size: 0,
        })
    })
}

pub fn init_events(dir: impl AsRef<Path>) {
    let dir = dir.as_ref().to_path_buf();
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let path = dir.join(EVENTS_FILE);
    let Ok(file) = OpenOptions::new().create(true).append(true).open(&path) else {
        return;
    };
    let size = file.metadata().map(|m| m.len()).unwrap_or_default();
    if let Ok(mut st) = events_state().lock() {
        st.dir = Some(dir);
        st.file = Some(file);
        st.size = size;
    }
}

/// 记录一条事件（失败静默）。
pub fn event(scope: &str, message: impl Display) {
    let text = redact(&message.to_string()).replace('\n', " | ");
    let line = format!("{} [{scope}] {}\n", timestamp(), truncate(&text, 4096));
    let Ok(mut st) = events_state().lock() else {
        return;
    };
    let Some(dir) = st.dir.clone() else {
        return;
    };
    if st.size + line.len() as u64 > EVENTS_MAX_BYTES {
        st.file = None;
        let base = dir.join(EVENTS_FILE);
        let _ = std::fs::remove_file(dir.join(format!("{EVENTS_FILE}.2")));
        let _ = std::fs::rename(
            dir.join(format!("{EVENTS_FILE}.1")),
            dir.join(format!("{EVENTS_FILE}.2")),
        );
        let _ = std::fs::rename(&base, dir.join(format!("{EVENTS_FILE}.1")));
        st.file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&base)
            .ok();
        st.size = 0;
    }
    if let Some(file) = st.file.as_mut()
        && file.write_all(line.as_bytes()).is_ok()
    {
        st.size += line.len() as u64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_json_and_key_value() {
        let input = r#"{"network_secret":"abc123","hostname":"n1"}"#;
        let out = redact(input);
        assert!(!out.contains("abc123"));
        assert!(out.contains("\"network_secret\":\"***\""));
        assert!(out.contains("n1"));

        let out = redact("password=hunter2 token: xyz");
        assert!(!out.contains("hunter2"));
        assert!(!out.contains("xyz"));
    }

    #[test]
    fn keeps_non_sensitive_lookalikes() {
        let out = redact("password_hash=abc");
        assert!(out.contains("abc"));
    }

    #[test]
    fn timestamp_format_is_stable() {
        // 2026-11-03T00:00:00Z == 1793664000
        let (y, m, d) = civil_from_days(1_793_664_000 / 86_400);
        assert_eq!((y, m, d), (2026, 11, 3));
    }

    #[test]
    fn truncate_respects_char_boundary() {
        let s = "中".repeat(10);
        let out = truncate(&s, 7);
        assert!(out.starts_with("中中"));
    }
}
