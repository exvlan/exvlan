/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Luis Liu. All rights reserved.
 *  Licensed under the MIT License. See License in the project root for license information.
 *--------------------------------------------------------------------------------------------*/

//! Linux 提权：`pkexec`（polkit）优先，失败回落 `sudo -A` + 随包 askpass 脚本。
//!
//! 两条路径都不需要我们自己处理密码：pkexec 由 polkit agent 弹窗，`sudo -A`
//! 由 `SUDO_ASKPASS` 指向的脚本弹窗（zenity / kdialog / xterm），密码只在这两个
//! 进程的 stdin/stdout 之间流动，不落盘、不进日志。
//!
//! 两者都不可用时，错误信息里带上**可直接复制的命令**，由界面展示给用户。

use super::Command;
use anyhow::{Result, anyhow};
use std::env;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command as StdCommand, Output, Stdio};

/// 可用的提权工具。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tool {
    /// `pkexec <program> <args...>`：polkit 弹窗，桌面默认路径。
    Pkexec,
    /// `sudo -A <program> <args...>` + `SUDO_ASKPASS`：没有 polkit agent 时的回落。
    SudoAskpass,
}

/// 选提权工具（**纯逻辑**，便于单测）。
///
/// - 有 `pkexec` 就用它；
/// - 否则回落 `sudo -A`，但没有 askpass 脚本时 `sudo` 无法弹窗输入密码 → 视为不可用。
pub(crate) fn choose_tool(pkexec: bool, sudo: bool, askpass: bool) -> Option<Tool> {
    if pkexec {
        Some(Tool::Pkexec)
    } else if sudo && askpass {
        Some(Tool::SudoAskpass)
    } else {
        None
    }
}

/// 在 `PATH` 里找可执行文件（不写死绝对路径，也不新增依赖）。
fn find_in_path(name: &str) -> Option<PathBuf> {
    let paths = env::var_os("PATH")?;
    env::split_paths(&paths)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// POSIX shell 单引号转义，供「可复制命令」提示使用。
pub(crate) fn shell_quote(value: &str) -> String {
    let plain = !value.is_empty()
        && value.chars().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | ':' | '=' | '@' | '+')
        });
    if plain {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', r"'\''"))
    }
}

/// 提权不可用时给用户的手动命令（等价于我们尝试做的事）。
pub(crate) fn passthrough_command(program: &Path, args: &[OsString]) -> String {
    let mut out = format!("sudo {}", shell_quote(&program.to_string_lossy()));
    for arg in args {
        out.push(' ');
        out.push_str(&shell_quote(&arg.to_string_lossy()));
    }
    out
}

/// `pkexec` 的参数拼装。
///
/// `--disable-internal-agent`：只走桌面 polkit agent，避免 pkexec 自己在 tty 上
/// 起文本提示（GUI 进程没有可用的 tty）。`env` 前缀保留图形会用到的变量。
fn pkexec_command(program: &Path, args: &[OsString], envs: &[OsString]) -> StdCommand {
    let mut command = StdCommand::new("pkexec");
    command.arg("--disable-internal-agent");
    if !envs.is_empty() {
        command.arg("env");
        command.args(envs);
    }
    command.arg(program).args(args);
    command.stdin(Stdio::null());
    command
}

/// `sudo -A` 的参数拼装：密码由 `SUDO_ASKPASS` 指向的脚本弹窗输入。
///
/// `env` 前缀与 pkexec 分支保持一致：`sudo` 默认重置环境，`HOME` 会变成 root 的。
/// 而迁移、服务安装都靠 `HOME`/`XDG_*` 定位用户目录 —— 丢了就会「看不见用户的旧数据」，
/// 迁移直接报「无需迁移」并退出 1，界面上却表现为永久卡在迁移闸门。
fn sudo_command(
    program: &Path,
    args: &[OsString],
    askpass: &Path,
    envs: &[OsString],
) -> StdCommand {
    let mut command = StdCommand::new("sudo");
    command
        .arg("-A")
        .arg("env")
        .args(envs)
        .arg("--")
        .arg(program)
        .args(args)
        .env("SUDO_ASKPASS", askpass)
        .stdin(Stdio::null());
    command
}

/// pkexec 的这个退出码代表「没授权 / 没有 agent」→ 值得用 sudo 再试一次；
/// 其它退出码是**业务失败**（例如 `--install` 自己失败），原样返回给上层。
fn should_fall_back(code: Option<i32>) -> bool {
    matches!(code, None | Some(126) | Some(127))
}

fn run_sudo(
    program: &Path,
    args: &[OsString],
    askpass: &Path,
    envs: &[OsString],
) -> Result<Output> {
    crate::logger::record(
        "elevate",
        format!(
            "sudo -A {} {}",
            program.display(),
            args.iter()
                .map(|a| a.to_string_lossy().to_string())
                .collect::<Vec<_>>()
                .join(" ")
        ),
    );
    sudo_command(program, args, askpass, envs)
        .output()
        .map_err(|err| anyhow!("无法启动 sudo: {err}"))
}

/// 提权不可用时的错误（带可复制命令）。
fn unavailable(reason: &str, program: &Path, args: &[OsString]) -> anyhow::Error {
    anyhow!(
        "{reason}\n请在终端手动执行（复制即可）：\n  {}",
        passthrough_command(program, args)
    )
}

/// The implementation of state check and elevated executing varies on each platform
impl Command {
    /// Check the state the current program running
    ///
    /// Return `true` if the program is running as root, otherwise false
    pub fn is_elevated() -> bool {
        let uid = unsafe { libc::getuid() };
        uid == 0
    }

    /// Prompting the user with a graphical OS dialog for the root password,
    /// excuting the command with escalated privileges, and return the output
    pub fn output(&self) -> Result<Output> {
        let program = {
            let raw = self.cmd.get_program();
            let path = Path::new(raw);
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                find_in_path(&raw.to_string_lossy())
                    .ok_or_else(|| anyhow!("找不到可执行文件: {}", raw.to_string_lossy()))?
            }
        };
        let args: Vec<OsString> = self.cmd.get_args().map(OsStr::to_os_string).collect();

        // 已经是 root（例如用户从终端 `sudo` 启动 GUI）：再套一层 pkexec/sudo 只会失败。
        if Self::is_elevated() {
            return StdCommand::new(&program)
                .args(&args)
                .stdin(Stdio::null())
                .output()
                .map_err(|err| anyhow!("无法执行 {}: {err}", program.display()));
        }

        // 保留图形环境变量（沿用旧行为：DISPLAY / XAUTHORITY / HOME + 显式 env）。
        let mut envs: Vec<OsString> = Vec::new();
        for key in ["DISPLAY", "XAUTHORITY", "HOME"] {
            if let Some(value) = env::var_os(key) {
                let mut pair = OsString::from(key);
                pair.push("=");
                pair.push(value);
                envs.push(pair);
            }
        }
        for (key, value) in self.cmd.get_envs() {
            if let Some(value) = value {
                let mut pair = OsString::from(key);
                pair.push("=");
                pair.push(value);
                envs.push(pair);
            }
        }

        let has_pkexec = find_in_path("pkexec").is_some();
        let has_sudo = find_in_path("sudo").is_some();
        let askpass = super::askpass_path();

        let Some(tool) = choose_tool(has_pkexec, has_sudo, askpass.is_some()) else {
            return Err(unavailable(
                "未找到可用的图形化提权方式（缺少 pkexec，或 sudo 缺少 askpass 脚本）",
                &program,
                &args,
            ));
        };
        crate::logger::record(
            "elevate",
            format!("tool={tool:?} program={} args={args:?}", program.display()),
        );

        match tool {
            Tool::Pkexec => {
                match pkexec_command(&program, &args, &envs).output() {
                    Ok(out) if out.status.success() => Ok(out),
                    // 业务失败（安装脚本自己报错）：原样返回，别再用 sudo 重弹一次密码。
                    Ok(out) if !should_fall_back(out.status.code()) => Ok(out),
                    other => {
                        crate::logger::record(
                            "elevate",
                            format!(
                                "pkexec 不可用（{:?}），回落 sudo -A",
                                match &other {
                                    Ok(out) => format!("exit={}", out.status),
                                    Err(err) => err.to_string(),
                                }
                            ),
                        );
                        match (has_sudo, askpass) {
                            (true, Some(askpass)) => run_sudo(&program, &args, &askpass, &envs),
                            _ => match other {
                                Ok(out) => Ok(out),
                                Err(err) => Err(unavailable(
                                    &format!("pkexec 失败（{err}），且没有可用的 sudo 回落"),
                                    &program,
                                    &args,
                                )),
                            },
                        }
                    }
                }
            }
            Tool::SudoAskpass => match askpass {
                Some(askpass) => run_sudo(&program, &args, &askpass, &envs),
                // choose_tool 已保证有脚本；这里不 panic，退回可复制命令。
                None => Err(unavailable(
                    "未找到 askpass 脚本（resources/tools/exvlan-askpass.sh）",
                    &program,
                    &args,
                )),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_pkexec_when_available() {
        assert_eq!(choose_tool(true, true, true), Some(Tool::Pkexec));
        assert_eq!(choose_tool(true, false, false), Some(Tool::Pkexec));
    }

    #[test]
    fn falls_back_to_sudo_askpass() {
        assert_eq!(choose_tool(false, true, true), Some(Tool::SudoAskpass));
    }

    #[test]
    fn sudo_without_askpass_is_unusable() {
        // 有 sudo 但没 askpass 脚本 → 无法弹窗，等同于不可提权。
        assert_eq!(choose_tool(false, true, false), None);
        assert_eq!(choose_tool(false, false, false), None);
    }

    #[test]
    fn sudo_command_points_askpass_at_script() {
        let askpass = std::env::temp_dir().join("exvlan-askpass.sh");
        let program = std::env::temp_dir().join("exvlan-service");
        let args = vec![OsString::from("--install"), OsString::from("1000")];

        // sudo 会重置环境：HOME 必须显式带过去，否则子进程按 root 的用户目录找旧数据。
        let envs = vec![
            OsString::from("HOME=/home/alice"),
            OsString::from("DISPLAY=:0"),
        ];
        let command = sudo_command(&program, &args, &askpass, &envs);
        let child_envs: Vec<(String, Option<OsString>)> = command
            .get_envs()
            .map(|(k, v)| (k.to_string_lossy().to_string(), v.map(OsStr::to_os_string)))
            .collect();
        assert_eq!(
            child_envs,
            vec![("SUDO_ASKPASS".to_string(), Some(askpass.into_os_string()))]
        );

        let argv: Vec<String> = command
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert_eq!(argv[0], "-A");
        assert_eq!(argv[1], "env");
        assert_eq!(argv[2], "HOME=/home/alice");
        assert_eq!(argv[3], "DISPLAY=:0");
        assert_eq!(argv[4], "--");
        assert_eq!(argv[5], program.display().to_string());
        assert_eq!(&argv[6..], &["--install".to_string(), "1000".to_string()]);
    }

    #[test]
    fn passthrough_command_is_copy_pasteable() {
        let program = std::env::temp_dir().join("bin dir").join("exvlan-service");
        let args = vec![OsString::from("--install"), OsString::from("1000")];
        let command = passthrough_command(&program, &args);
        // 含空格的路径要加引号，参数原样保留。
        assert!(command.starts_with("sudo '"), "{command}");
        assert!(command.ends_with("--install 1000"), "{command}");
        assert_eq!(shell_quote("--tools-dir"), "--tools-dir");
    }
}
