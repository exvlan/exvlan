//! `service.toml` 里的非法 `owner_sid` 存量清洗（pre.17 的 dev.1 事故）。
//!
//! 与 pre.17 迁移**解耦**：全新安装的机器也要跑（dev.1 的脏 `owner_sid` 正是
//! 「全新安装 + `--install --tools-dir`」产生的），所以它有自己的标记
//! [`crate::SANITIZE_MARKER`]，并且在「无旧数据即退出」之前执行。
//!
//! 只改一行值：注释 / 字段顺序 / 行尾都原样保留（这份文件是 root 手工排查时
//! 唯一的现场）。

use std::path::Path;

use anyhow::Context as _;
use serde_json::json;

use crate::{Host, Level, SANITIZE_MARKER, TOOL_VERSION};

/// 一次性清洗 `service.toml` 里的 `owner_sid`。
///
/// 非法判据与写入侧同一份（[`exvlan_control::identity::valid_owner_sid`]）：
/// 只认 `S-1-...`，空串 / `--tools-dir` 这类安装参数残渣一律抹成空串。
pub fn service_config(
    dry_run: bool,
    data: &Path,
    config: &Path,
    host: &mut dyn Host,
) -> anyhow::Result<()> {
    let marker = data.join(SANITIZE_MARKER);
    if marker.is_file() {
        host.log(
            Level::Info,
            &format!("sanitize: {} 已存在，跳过", marker.display()),
        );
        return Ok(());
    }
    let text = match std::fs::read_to_string(config) {
        Ok(text) => text,
        // 还没装服务：没有可清洗的，写标记免得每次启动都再读一遍。
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            host.log(
                Level::Info,
                &format!("sanitize: {} 不存在，无需清洗", config.display()),
            );
            if !dry_run {
                write_marker(&marker, false)?;
            }
            return Ok(());
        }
        Err(err) => return Err(err).with_context(|| format!("读取 {} 失败", config.display())),
    };
    let (cleaned, changed) = strip_invalid_owner_sid(&text);
    if !changed {
        host.log(
            Level::Info,
            "sanitize: service.toml 的 owner_sid 合法，无需清洗",
        );
        if !dry_run {
            write_marker(&marker, false)?;
        }
        return Ok(());
    }
    if dry_run {
        host.log(
            Level::Warn,
            &format!(
                "sanitize: 将清空 {} 里非法的 owner_sid（加 --yes 执行）",
                config.display()
            ),
        );
        return Ok(());
    }
    // 临时文件 + rename：服务可能正在读同一份文件（原权限 0600，属主 root）。
    let tmp = config.with_extension("toml.new");
    std::fs::write(&tmp, &cleaned).with_context(|| format!("写入 {} 失败", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600)).ok();
    }
    std::fs::rename(&tmp, config).with_context(|| format!("替换 {} 失败", config.display()))?;
    host.log(
        Level::Info,
        &format!("sanitize: 已清空 {} 里非法的 owner_sid", config.display()),
    );
    write_marker(&marker, true)?;
    Ok(())
}

fn write_marker(marker: &Path, cleared: bool) -> anyhow::Result<()> {
    if let Some(dir) = marker.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    std::fs::write(
        marker,
        serde_json::to_string_pretty(&json!({
            "version": TOOL_VERSION,
            "ts": exvlan_control::tokens::now_secs(),
            "owner_sid_cleared": cleared,
        }))?,
    )
    .with_context(|| format!("写 {} 失败", marker.display()))
}

/// 把 `owner_sid = "<非法值>"` 换成 `owner_sid = ""`，**其余字节原样保留**
/// （注释 / 字段顺序 / 行尾都必须不动）。
///
/// 返回 `(新正文, 是否有改动)`；没有改动时正文与输入完全一致。
pub fn strip_invalid_owner_sid(text: &str) -> (String, bool) {
    let mut out = String::with_capacity(text.len());
    let mut changed = false;
    for line in text.split_inclusive('\n') {
        let (body, eol) = split_eol(line);
        match owner_sid_span(body) {
            // 已经是空串 = 无需清洗（否则每次都会「改动」一次，破坏幂等）。
            Some((start, end, value))
                if !value.trim().is_empty()
                    && exvlan_control::identity::valid_owner_sid(&value).is_none() =>
            {
                out.push_str(&body[..start]);
                out.push_str("\"\"");
                out.push_str(&body[end..]);
                out.push_str(eol);
                changed = true;
            }
            _ => out.push_str(line),
        }
    }
    (out, changed)
}

/// 拆行尾（`\r\n` / `\n` / 无），用于逐行改写时原样回填。
fn split_eol(line: &str) -> (&str, &str) {
    if let Some(body) = line.strip_suffix("\r\n") {
        (body, "\r\n")
    } else if let Some(body) = line.strip_suffix('\n') {
        (body, "\n")
    } else {
        (line, "")
    }
}

/// 定位一行里 `owner_sid` 的**值区间**：`(起始字节, 结束字节, 值文本)`。
///
/// 只认「整行就是 `owner_sid = ...` 这个键」（行首可以是缩进），不碰注释或
/// 其它键的值里恰好出现的同名字样。
fn owner_sid_span(line: &str) -> Option<(usize, usize, String)> {
    const KEY: &str = "owner_sid";
    let key_at = line.find(KEY)?;
    if !line[..key_at].trim().is_empty() {
        return None;
    }
    let after_key = &line[key_at + KEY.len()..];
    let after_eq = after_key.trim_start();
    let after_eq = after_eq.strip_prefix('=')?;
    let value_start = line.len() - after_eq.len();
    let rest = &line[value_start..];
    let trimmed = rest.trim_start();
    let start = value_start + (rest.len() - trimmed.len());
    if trimmed.is_empty() {
        return None;
    }
    match trimmed.as_bytes()[0] {
        quote @ (b'"' | b'\'') => {
            let body = &trimmed[1..];
            let close = body.find(quote as char)?;
            let end = start + 1 + close + 1;
            Some((start, end, body[..close].to_string()))
        }
        // 裸值（TOML 里非法，但历史文件可能写成 `owner_sid = --tools-dir`）
        _ => {
            let len = trimmed
                .find(|c: char| c.is_whitespace() || c == '#')
                .unwrap_or(trimmed.len());
            Some((start, start + len, trimmed[..len].to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NoHost;
    use std::path::PathBuf;

    fn temp(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "exvlan-migrate-sanitize-{tag}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn strips_installer_flag_but_keeps_everything_else() {
        // dev.1 真机上的原始脏值形状：macOS 的 args[2] 是 `--tools-dir`。
        let dirty = "# 注释\npolicy = \"resident\"\nowner_sid = \"--tools-dir\"\n\n[gateway]\nenabled = true\n";
        let (clean, changed) = strip_invalid_owner_sid(dirty);
        assert!(changed);
        assert_eq!(
            clean,
            "# 注释\npolicy = \"resident\"\nowner_sid = \"\"\n\n[gateway]\nenabled = true\n"
        );
        // 幂等：再跑一次不再改动
        let (again, changed) = strip_invalid_owner_sid(&clean);
        assert!(!changed);
        assert_eq!(again, clean);
    }

    #[test]
    fn leaves_valid_owner_sid_and_crlf_alone() {
        let ok = "owner_sid = \"S-1-5-21-1004336348-1177238915-682003330-1001\"\r\npolicy = \"session\"\r\n";
        let (out, changed) = strip_invalid_owner_sid(ok);
        assert!(!changed);
        assert_eq!(out, ok);

        // 行尾注释保留，只替换值；CRLF 不能变成 LF。
        let dirty = "owner_sid = \"root\" # 历史脏值\r\npolicy = \"session\"\r\n";
        let (out, changed) = strip_invalid_owner_sid(dirty);
        assert!(changed);
        assert_eq!(
            out,
            "owner_sid = \"\" # 历史脏值\r\npolicy = \"session\"\r\n"
        );
    }

    #[test]
    fn ignores_owner_sid_mentions_elsewhere() {
        // 别的键的值 / 注释里出现 owner_sid 时不能误改。
        let text = "# owner_sid = \"--tools-dir\"\nrpc_portal = \"owner_sid\"\n";
        let (out, changed) = strip_invalid_owner_sid(text);
        assert!(!changed);
        assert_eq!(out, text);
    }

    #[test]
    fn sanitize_rewrites_config_then_marks_done() {
        let user = temp("mark-user");
        let sys = temp("mark-sys");
        let config = sys.join("service.toml");
        std::fs::write(
            &config,
            "owner_sid = \"--tools-dir\"\npolicy = \"resident\"\n",
        )
        .unwrap();

        service_config(false, &user, &config, &mut NoHost).expect("sanitize");

        let text = std::fs::read_to_string(&config).unwrap();
        assert_eq!(text, "owner_sid = \"\"\npolicy = \"resident\"\n");
        assert!(user.join(SANITIZE_MARKER).is_file(), "应写 migrate.done.v2");

        // 标记存在 → 不再碰文件（即使文件又被写脏）
        std::fs::write(&config, "owner_sid = \"--tools-dir\"\n").unwrap();
        service_config(false, &user, &config, &mut NoHost).expect("skip");
        assert_eq!(
            std::fs::read_to_string(&config).unwrap(),
            "owner_sid = \"--tools-dir\"\n"
        );

        let _ = std::fs::remove_dir_all(&user);
        let _ = std::fs::remove_dir_all(&sys);
    }

    #[test]
    fn sanitize_dry_run_does_not_touch_file() {
        let user = temp("dry-user");
        let sys = temp("dry-sys");
        let config = sys.join("service.toml");
        let dirty = "owner_sid = \"--tools-dir\"\n";
        std::fs::write(&config, dirty).unwrap();

        service_config(true, &user, &config, &mut NoHost).expect("dry-run");

        assert_eq!(std::fs::read_to_string(&config).unwrap(), dirty);
        assert!(!user.join(SANITIZE_MARKER).is_file(), "dry-run 不写标记");

        let _ = std::fs::remove_dir_all(&user);
        let _ = std::fs::remove_dir_all(&sys);
    }

    #[test]
    fn missing_config_is_marked_without_touching_anything() {
        let user = temp("missing-user");
        let config = user.join("nope").join("service.toml");
        service_config(false, &user, &config, &mut NoHost).expect("ok");
        assert!(user.join(SANITIZE_MARKER).is_file(), "没装服务也要落标记");
        assert!(!config.exists());
        let _ = std::fs::remove_dir_all(&user);
    }
}
