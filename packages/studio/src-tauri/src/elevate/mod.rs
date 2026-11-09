#![allow(dead_code)]
/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Luis Liu. All rights reserved.
 *  Licensed under the MIT License. See License in the project root for license information.
 *--------------------------------------------------------------------------------------------*/
use std::convert::From;
use std::process::Command as StdCommand;

/// Wrap of std::process::command and escalate privileges while executing
pub struct Command {
    cmd: StdCommand,
    #[allow(dead_code)]
    icon: Option<Vec<u8>>,
    #[allow(dead_code)]
    name: Option<String>,
}

/// Command initialization shares the same logic across all the platforms
impl Command {
    /// Constructs a new `Command` from a std::process::Command
    /// instance, it would read the following configuration from
    /// the instance while executing:
    ///
    /// * The instance's path to the program
    /// * The instance's arguments
    /// * The instance's environment variables
    ///
    /// So far, the new `Command` would only take the environment variables explicitly
    /// set by std::process::Command::env and std::process::Command::env,
    /// without the ones inherited from the parent process
    ///
    /// And the environment variables would only be taken on Linux and MacOS,
    /// they would be ignored on Windows
    ///
    /// Current working directory would be the following while executing the command:
    ///   - %SystemRoot%\System32 on Windows
    ///   - /root on Linux
    ///   - $TMPDIR/sudo_prompt_applet/applet.app/Contents/MacOS on MacOS
    ///
    /// To pass environment variables on Windows,
    /// to inherit environment variables from the parent process and
    /// to change the working directory will be supported in later versions
    pub fn new(cmd: StdCommand) -> Self {
        Self {
            cmd,
            icon: None,
            name: None,
        }
    }

    /// Consumes the `Take`, returning the wrapped std::process::Command
    ///
    /// # Examples
    pub fn into_inner(self) -> StdCommand {
        self.cmd
    }

    /// Gets a mutable reference to the underlying std::process::Command
    pub fn get_ref(&self) -> &StdCommand {
        &self.cmd
    }

    /// Gets a reference to the underlying std::process::Command
    pub fn get_mut(&mut self) -> &mut StdCommand {
        &mut self.cmd
    }

    /// Set the `icon` for the pop-up graphical OS dialog
    pub fn icon(&mut self, icon: Vec<u8>) -> &mut Self {
        self.icon = Some(icon);
        self
    }

    /// Set the name for the pop-up graphical OS dialog
    ///
    /// This method is only applicable on `MacOS`
    pub fn name(&mut self, name: String) -> &mut Self {
        self.name = Some(name);
        self
    }
}

impl From<StdCommand> for Command {
    /// Converts from a std::process::Command
    ///
    /// It is similiar with the construct method
    fn from(cmd: StdCommand) -> Self {
        Self {
            cmd,
            icon: None,
            name: None,
        }
    }
}

/// 按 Windows `CommandLineToArgvW` 的规则把参数拼成命令行。
///
/// 直接 `args.join(" ")` 会把 JSON/带空格路径拆碎（历史上导致
/// `[service] 参数解析失败: key must be a string at line 1 column 2`）。
pub fn quote_args<I: IntoIterator<Item = String>>(args: I) -> String {
    args.into_iter()
        .map(|a| quote_arg(&a))
        .collect::<Vec<_>>()
        .join(" ")
}

/// 单个参数的引号化；规则见
/// <https://learn.microsoft.com/en-us/cpp/cpp/main-function-command-line-args>。
pub fn quote_arg(arg: &str) -> String {
    let needs_quote = arg.is_empty()
        || arg
            .chars()
            .any(|c| matches!(c, ' ' | '\t' | '\n' | '\u{0b}' | '"'));
    if !needs_quote {
        return arg.to_string();
    }

    let mut out = String::with_capacity(arg.len() + 2);
    out.push('"');
    let mut backslashes = 0usize;
    for ch in arg.chars() {
        match ch {
            '\\' => backslashes += 1,
            '"' => {
                out.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            _ => {
                out.extend(std::iter::repeat_n('\\', backslashes));
                backslashes = 0;
                out.push(ch);
            }
        }
    }
    out.extend(std::iter::repeat_n('\\', backslashes * 2));
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_args_per_windows_rules() {
        // 无特殊字符 → 不加引号
        assert_eq!(quote_arg("--provision"), "--provision");
        assert_eq!(
            quote_arg(r"C:\Apps\era.exvlan.studio"),
            r"C:\Apps\era.exvlan.studio"
        );

        // 含空格 → 加引号
        assert_eq!(
            quote_arg("C:\\Program Files\\a b.exe"),
            "\"C:\\Program Files\\a b.exe\""
        );

        // 内嵌引号 → 前面补反斜杠
        assert_eq!(quote_arg(r#"{"a":"b"}"#), r#""{\"a\":\"b\"}""#);

        // 含空格且结尾反斜杠 → 引号内的反斜杠成对加倍
        assert_eq!(
            quote_arg(r"C:\Program Files\foo\"),
            r#""C:\Program Files\foo\\""#
        );
        // 不含空格时即便结尾是反斜杠也不需要引号
        assert_eq!(quote_arg(r"C:\tmp\"), r"C:\tmp\");

        // 空参数也要引号
        assert_eq!(quote_arg(""), "\"\"");

        // 多个参数拼接
        assert_eq!(
            quote_args(vec!["--provision".to_string(), r#"{"a":"b"}"#.to_string()]),
            r#"--provision "{\"a\":\"b\"}""#
        );
    }
}

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;
