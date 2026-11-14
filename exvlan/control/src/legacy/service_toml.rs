//! `service.toml` 旧字段名 / 旧词汇归一（S8）。
//!
//! 为什么需要：`exvlan/service/src/main.rs` 的 `ServiceConfig::policy` 带
//! `#[serde(alias = "mode")]`，`Mode` 还带 `service|Service` / `normal|Normal` /
//! `off|Off` 这套旧词汇。别名存在的唯一理由就是「更早的 pre.17 构建写过旧形状」。
//! 按铁律（`docs/decisions-7.3.md` §14.15）别名要删，但**删之前**必须先把存量归一，
//! 否则 `load_config()` 的 `.ok().unwrap_or_default()` 会把策略静默回落成
//! `resident` —— 用户的服务策略被悄悄改掉，比崩溃更难查。
//!
//! 归一只做两件事，其余字节原样保留（注释 / 字段顺序 / 行尾都不动）：
//! 1. 顶层 `mode = <值>` → `policy = <值>`；
//! 2. 值按现有词汇归一：`service|Service` → `resident`、`normal|Normal` → `session`、
//!    `off|Off` → `stopped`；已是 `resident|session|stopped` 的保持不动。
//!
//! 逐行文本改写（而不是解析成 `toml::Value` 再序列化）是**刻意的**：重新序列化会丢掉
//! 注释和字段顺序，而这份文件是 root 手工排查时唯一的现场。

use std::path::Path;

/// 归一结果。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Normalized {
    /// 归一后的全文（`changed == false` 时与原文本逐字相同）。
    pub text: String,
    pub changed: bool,
    /// 顶层 `mode` 改名为 `policy` 的次数（0/1）。
    pub renames: usize,
    /// 值被改写的处数。
    pub values_fixed: usize,
    /// 同时存在 `mode` 与 `policy` 时被丢弃的 `mode` 行。
    pub dropped_mode: bool,
    /// 认不出的取值（保持原样，只记录，便于人工复查）。
    pub unknown_values: Vec<String>,
}

/// 归一后的词汇；`None` 表示不认识（保持原样）。
pub fn normalize_value(raw: &str) -> Option<&'static str> {
    match raw {
        "resident" | "session" | "stopped" => None,
        "service" | "Service" => Some("resident"),
        "normal" | "Normal" => Some("session"),
        "off" | "Off" => Some("stopped"),
        _ => None,
    }
}

/// 是否需要归一（检测口径，与 [`normalize`] 同一份实现）。
pub fn needs_normalization(text: &str) -> bool {
    normalize(text).changed
}

#[derive(Debug, Clone, Copy)]
struct Assignment {
    key_start: usize,
    key_end: usize,
    value_start: usize,
    value_end: usize,
}

/// 顶层（未被 `[section]` 覆盖）的 `key = value` 行定位。
fn assignment(line: &str, key: &str) -> Option<Assignment> {
    let key_at = line.find(key)?;
    if !line[..key_at].trim().is_empty() {
        return None;
    }
    let after_key = &line[key_at + key.len()..];
    let after_eq = after_key.trim_start();
    let after_eq = after_eq.strip_prefix('=')?;
    let value_start = line.len() - after_eq.len();
    let rest = &line[value_start..];
    let trimmed = rest.trim_start();
    let start = value_start + (rest.len() - trimmed.len());
    if trimmed.is_empty() {
        return None;
    }
    let end = match trimmed.as_bytes()[0] {
        quote @ (b'"' | b'\'') => {
            let body = &trimmed[1..];
            let close = body.find(quote as char)?;
            start + 1 + close + 1
        }
        _ => {
            let len = trimmed
                .find(|c: char| c.is_whitespace() || c == '#')
                .unwrap_or(trimmed.len());
            if len == 0 {
                return None;
            }
            start + len
        }
    };
    Some(Assignment {
        key_start: key_at,
        key_end: key_at + key.len(),
        value_start: start,
        value_end: end,
    })
}

/// 值文本（去掉引号，不含行尾注释）。
fn value_of<'a>(line: &'a str, span: &Assignment) -> &'a str {
    let raw = &line[span.value_start..span.value_end];
    if raw.len() >= 2 {
        let bytes = raw.as_bytes();
        if (bytes[0] == b'"' || bytes[0] == b'\'') && bytes[bytes.len() - 1] == bytes[0] {
            return &raw[1..raw.len() - 1];
        }
    }
    raw
}

fn split_eol(line: &str) -> (&str, &str) {
    if let Some(body) = line.strip_suffix("\r\n") {
        (body, "\r\n")
    } else if let Some(body) = line.strip_suffix('\n') {
        (body, "\n")
    } else {
        (line, "")
    }
}

/// 归一全文。
pub fn normalize(text: &str) -> Normalized {
    // 先看顶层是否已经有 `policy`（同时存在时保留 policy，丢掉旧 mode 行）。
    let mut has_policy = false;
    let mut in_section = false;
    for line in text.split_inclusive('\n') {
        let body = split_eol(line).0;
        if is_section_header(body) {
            in_section = true;
            continue;
        }
        if !in_section && assignment(body, "policy").is_some() {
            has_policy = true;
        }
    }

    let mut out = String::with_capacity(text.len());
    let mut result = Normalized::default();
    in_section = false;
    for line in text.split_inclusive('\n') {
        let (body, eol) = split_eol(line);
        if is_section_header(body) {
            in_section = true;
            out.push_str(line);
            continue;
        }
        if in_section {
            out.push_str(line);
            continue;
        }
        // 1) `mode` → `policy`（现有 `policy` 已存在时丢掉旧行，避免 serde 重复字段）
        if let Some(span) = assignment(body, "mode") {
            if has_policy {
                result.dropped_mode = true;
                result.changed = true;
                continue;
            }
            let mut rebuilt = String::with_capacity(body.len() + 2);
            rebuilt.push_str(&body[..span.key_start]);
            rebuilt.push_str("policy");
            rebuilt.push_str(&body[span.key_end..]);
            result.renames += 1;
            result.changed = true;
            out.push_str(&rewrite_value_if_needed(&rebuilt, &mut result));
            out.push_str(eol);
            continue;
        }
        // 2) `policy` 的旧词汇
        if assignment(body, "policy").is_some() {
            out.push_str(&rewrite_value_if_needed(body, &mut result));
            out.push_str(eol);
            continue;
        }
        out.push_str(line);
    }
    result.text = out;
    result
}

fn is_section_header(body: &str) -> bool {
    body.trim_start().starts_with('[')
}

fn rewrite_value_if_needed(line: &str, result: &mut Normalized) -> String {
    let Some(span) = assignment(line, "policy") else {
        return line.to_string();
    };
    let current = value_of(line, &span).to_string();
    match normalize_value(&current) {
        Some(new) => {
            result.values_fixed += 1;
            result.changed = true;
            let mut out = String::with_capacity(line.len());
            out.push_str(&line[..span.value_start]);
            out.push_str(&format!("\"{new}\""));
            out.push_str(&line[span.value_end..]);
            out
        }
        None => {
            if !matches!(current.as_str(), "resident" | "session" | "stopped") {
                result.unknown_values.push(current);
            }
            line.to_string()
        }
    }
}

/// 读文件并归一；读不到时返回错误（调用方决定是否告警跳过）。
pub fn normalize_file(path: &Path) -> std::io::Result<Normalized> {
    let text = std::fs::read_to_string(path)?;
    let mut result = normalize(&text);
    if !result.changed {
        result.text = text;
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renames_mode_to_policy_and_keeps_value() {
        let text = "owner_uid = 501\nmode = \"session\"\nrpc_mesh_whitelist = true\n";
        let out = normalize(text);
        assert!(out.changed);
        assert_eq!(out.renames, 1);
        assert_eq!(out.values_fixed, 0);
        assert_eq!(
            out.text,
            "owner_uid = 501\npolicy = \"session\"\nrpc_mesh_whitelist = true\n"
        );
    }

    #[test]
    fn normalizes_legacy_value_vocabulary() {
        for (raw, want) in [
            ("service", "resident"),
            ("Service", "resident"),
            ("normal", "session"),
            ("Normal", "session"),
            ("off", "stopped"),
            ("Off", "stopped"),
        ] {
            let text = format!("mode = \"{raw}\"\n");
            let out = normalize(&text);
            assert_eq!(
                out.text,
                format!("policy = \"{want}\"\n"),
                "mode={raw} 应归一为 policy={want}"
            );
            assert_eq!(out.values_fixed, 1);
        }
    }

    #[test]
    fn already_new_format_is_untouched() {
        let text = "# 注释\npolicy = \"session\"\n\n[gateway]\nenabled = true\nallow = [\"x\"]\n";
        let out = normalize(text);
        assert!(!out.changed);
        assert_eq!(out.text, text);
        assert_eq!(out.renames, 0);
        assert!(out.unknown_values.is_empty());
    }

    #[test]
    fn keeps_comments_every_other_byte() {
        let text = "# 由 exvlan-service 写入\n# 手工改过也不要动\npolicy = \"off\" # 行尾注释\nowner_sid = \"\"\n\n[gateway]\nenabled = false\nmode = \"normal\"\n";
        let out = normalize(text);
        assert!(out.changed);
        // 只改目标行；`[gateway]` 里的 `mode` 不属于顶层字段，整段原样保留
        assert_eq!(
            out.text,
            "# 由 exvlan-service 写入\n# 手工改过也不要动\npolicy = \"stopped\" # 行尾注释\nowner_sid = \"\"\n\n[gateway]\nenabled = false\nmode = \"normal\"\n"
        );
    }

    #[test]
    fn section_scoped_mode_is_not_renamed() {
        let text = "[kernel]\nmode = \"normal\"\n";
        let out = normalize(text);
        assert!(!out.changed, "{}", out.text);
    }

    #[test]
    fn policy_wins_when_both_present() {
        // `#[serde(alias = "mode")]` 同时出现会报重复字段 → load_config 回落默认值，
        // 所以必须把旧的 `mode` 行丢掉（保留 `policy`）。
        let text = "mode = \"off\"\npolicy = \"session\"\n";
        let out = normalize(text);
        assert!(out.changed);
        assert!(out.dropped_mode);
        assert_eq!(out.text, "policy = \"session\"\n");
    }

    #[test]
    fn unknown_values_are_reported_not_guessed() {
        let text = "policy = \"Resident\"\n";
        let out = normalize(text);
        assert!(!out.changed);
        assert_eq!(out.unknown_values, vec!["Resident".to_string()]);
    }

    #[test]
    fn crlf_is_preserved() {
        let text = "owner_uid = 0\r\nmode = \"normal\"\r\nowner_sid = \"\"\r\n";
        let out = normalize(text);
        assert_eq!(
            out.text,
            "owner_uid = 0\r\npolicy = \"session\"\r\nowner_sid = \"\"\r\n"
        );
    }

    #[test]
    fn needs_normalization_matches_normalize() {
        assert!(needs_normalization("mode = \"off\"\n"));
        assert!(!needs_normalization("policy = \"stopped\"\n"));
    }
}
