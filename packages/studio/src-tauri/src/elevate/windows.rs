/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Luis Liu. All rights reserved.
 *  Licensed under the MIT License. See License in the project root for license information.
 *--------------------------------------------------------------------------------------------*/

use super::Command;
use anyhow::Result;
use std::mem;
use std::os::windows::process::ExitStatusExt;
use std::process::{ExitStatus, Output};
use winapi::shared::minwindef::{DWORD, LPVOID};
use winapi::um::processthreadsapi::{GetCurrentProcess, OpenProcessToken};
use winapi::um::securitybaseapi::GetTokenInformation;
use winapi::um::winnt::{HANDLE, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
use windows::core::{HSTRING, PCWSTR, w};

/// The implementation of state check and elevated executing varies on each platform
impl Command {
    /// Check the state the current program running
    ///
    /// Return `true` if the program is running as root, otherwise false
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use elevated_command::Command;
    ///
    /// fn main() {
    ///     let is_elevated = Command::is_elevated();
    ///
    /// }
    /// ```
    pub fn is_elevated() -> bool {
        // Thanks to https://stackoverflow.com/a/8196291
        unsafe {
            let mut current_token_ptr: HANDLE = mem::zeroed();
            let mut token_elevation: TOKEN_ELEVATION = mem::zeroed();
            let token_elevation_type_ptr: *mut TOKEN_ELEVATION = &mut token_elevation;
            let mut size: DWORD = 0;

            let result = OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut current_token_ptr);

            if result != 0 {
                let result = GetTokenInformation(
                    current_token_ptr,
                    TokenElevation,
                    token_elevation_type_ptr as LPVOID,
                    mem::size_of::<winapi::um::winnt::TOKEN_ELEVATION_TYPE>() as u32,
                    &mut size,
                );
                if result != 0 {
                    return token_elevation.TokenIsElevated != 0;
                }
            }
        }
        false
    }

    /// Prompting the user with a graphical OS dialog for the root password,
    /// excuting the command with escalated privileges, and return the output
    ///
    /// On Windows, according to https://learn.microsoft.com/en-us/windows/win32/api/shellapi/nf-shellapi-shellexecutew#return-value,
    /// Output.status.code() shoudl be greater than 32 if the function succeeds,
    /// otherwise the value indicates the cause of the failure
    ///
    /// On Windows, Output.stdout and Output.stderr will always be empty as of now
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use elevated_command::Command;
    /// use std::process::Command as StdCommand;
    ///
    /// fn main() {
    ///     let mut cmd = StdCommand::new("path to the application");
    ///     let elevated_cmd = Command::new(cmd);
    ///     let output = elevated_cmd.output().unwrap();
    /// }
    /// ```
    pub fn output(&self) -> Result<Output> {
        let args = self
            .cmd
            .get_args()
            .map(|c| c.to_string_lossy().into_owned())
            .collect::<Vec<String>>();
        let parameters = if args.is_empty() {
            HSTRING::new()
        } else {
            // 必须按 Windows 规则转义：之前直接 join(" ") 会把 JSON 参数的引号吃掉，
            // 提权侧收到 `{a:b}` → "key must be a string at line 1 column 2"。
            HSTRING::from(super::quote_args(args))
        };

        use windows::Win32::Foundation::{CloseHandle, ERROR_CANCELLED};
        use windows::Win32::System::Threading::{
            GetExitCodeProcess, INFINITE, WaitForSingleObject,
        };
        use windows::Win32::UI::Shell::{
            SEE_MASK_FLAG_NO_UI, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
        };

        // 这两个 HSTRING 必须活到 ShellExecuteEx 返回之后再释放，
        // 否则 lpFile/lpParameters 会变成悬垂指针。
        let program = HSTRING::from(self.cmd.get_program());
        // 不用 ..Default::default()：该 impl 需要 Win32_System_Registry feature，
        // 这里零初始化后逐个赋值即可。
        let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
        info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_FLAG_NO_UI;
        info.lpVerb = w!("runas");
        info.lpFile = PCWSTR(program.as_ptr());
        info.lpParameters = PCWSTR(parameters.as_ptr());
        info.nShow = SW_HIDE.0;

        // ShellExecuteEx + NOCLOSEPROCESS：拿到进程句柄后**等待**子进程结束。
        // 旧实现用 ShellExecuteW 直接返回，父进程随即删掉临时目录，
        // 提权进程启动时内核 exe 已经不存在（[kernel] 安装失败: 内核源文件不存在）。
        if let Err(err) = unsafe { ShellExecuteExW(&mut info) } {
            // 用户点了 UAC 的“否”（ERROR_CANCELLED = 1223）
            // windows crate 把 Win32 错误包装成 HRESULT_FROM_WIN32，低 16 位才是原始错误码
            let code = err.code().0 as u32;
            if code & 0xFFFF == ERROR_CANCELLED.0 {
                anyhow::bail!("已取消管理员授权（ERROR_CANCELLED）");
            }
            anyhow::bail!("提权启动失败（Win32 错误 0x{code:08X}: {err}）");
        }
        if info.hProcess.is_invalid() {
            anyhow::bail!("提权进程句柄无效");
        }

        unsafe { WaitForSingleObject(info.hProcess, INFINITE) };
        let mut code: u32 = 1;
        let got = unsafe { GetExitCodeProcess(info.hProcess, &mut code) };
        unsafe { CloseHandle(info.hProcess) };
        if let Err(err) = got {
            anyhow::bail!("读取提权进程退出码失败: {err}");
        }

        Ok(Output {
            status: ExitStatus::from_raw(code),
            stdout: Vec::<u8>::new(),
            stderr: Vec::<u8>::new(),
        })
    }
}
