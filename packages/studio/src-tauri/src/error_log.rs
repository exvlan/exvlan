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
const MAX_BYTES: u64 = 5 * 1024 * 1024;
const KEEP_FILES: usize = 3;
const BUFFER_LIMIT: usize = 200;
const MAX_MESSAGE_BYTES: usize = 8 * 1024;

struct State {
    dir: Option<PathBuf>,
    file: Option<File>,
    size: u64,
    pending: Vec<String>,
}

fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(|| {
        Mutex::new(State {
            dir: None,
            file: None,
            size: 0,
            pending: Vec::new(),
        })
    })
}

/// 初始化（幂等）。`dir` 不存在时会被创建。
pub fn init(dir: impl AsRef<Path>) {
    let dir = dir.as_ref().to_path_buf();
    if let Err(err) = std::fs::create_dir_all(&dir) {
        eprintln!("[error_log] 创建日志目录失败（{}）: {err}", dir.display());
        return;
    }

    let path = dir.join(FILE_NAME);
    let file = match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(f) => f,
        Err(err) => {
            eprintln!("[error_log] 打开 {} 失败: {err}", path.display());
            return;
        }
    };
    let size = file.metadata().map(|m| m.len()).unwrap_or_default();

    let Ok(mut st) = state().lock() else {
        return;
    };
    st.dir = Some(dir);
    st.file = Some(file);
    st.size = size;

    // 回放初始化前积压的记录
    let pending = std::mem::take(&mut st.pending);
    for line in pending {
        st.write_line(&line);
    }
}

/// 当前日志文件路径（未初始化时为 `None`）。
pub fn path() -> Option<PathBuf> {
    state()
        .lock()
        .ok()
        .and_then(|st| st.dir.clone())
        .map(|d| d.join(FILE_NAME))
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

    match state().lock() {
        Ok(mut st) if st.file.is_some() => st.write_line(&line),
        Ok(mut st) => {
            if st.pending.len() >= BUFFER_LIMIT {
                st.pending.remove(0);
            }
            st.pending.push(line);
        }
        Err(_) => eprintln!("{}", line.trim_end()),
    }
}

impl State {
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
fn timestamp() -> String {
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
