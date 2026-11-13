//! 特权服务 IPC 客户端。
//!
//! 协议：unix socket（macOS/Linux）或 Windows 命名管道，**行式 JSON**：
//!
//! ```text
//! → {"proto":2,"op":"kernel.ensure"}
//! ← {"ok":true,"proto":2,"data":{"kind":"kernel.ensure","kernel":{...}}}
//! ```
//!
//! 请求/响应类型见 [`crate::protocol`]（唯一真源）。协议版本不匹配时服务端
//! 会硬拒并回 `proto_mismatch`，这里把它翻译成「请一起升级」的可读错误。
//!
//! 鉴权由操作系统负责（socket `0600` + 属主 / 命名管道 SDDL），这里不做自建鉴权。

use std::io::{BufRead as _, BufReader, Write};

use serde_json::Value;

use crate::paths;
use crate::protocol::{ErrorCode, PROTO, Request, Response, WireRequest};

/// 一条持久会话。
///
/// 「normal」策略下服务的生命周期靠它：断开即视为会话结束。
pub struct Client {
    writer: Box<dyn Write + Send>,
    reader: BufReader<Box<dyn std::io::Read + Send>>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("control::Client")
    }
}

impl Client {
    /// 连接服务（不发送 hello）。
    pub fn connect() -> Result<Self, String> {
        let addr = paths::service_ipc_addr();
        #[cfg(unix)]
        let stream = std::os::unix::net::UnixStream::connect(&addr)
            .map_err(|e| format!("连接 {} 失败: {e}", addr.display()))?;
        #[cfg(unix)]
        let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(30)));

        #[cfg(unix)]
        let (writer, reader) = {
            (
                Box::new(stream.try_clone().map_err(|e| e.to_string())?) as Box<dyn Write + Send>,
                BufReader::new(Box::new(stream) as Box<dyn std::io::Read + Send>),
            )
        };
        #[cfg(windows)]
        let (writer, reader) = {
            let file = open_pipe_with_retry(&addr)?;
            (
                Box::new(file.try_clone().map_err(|e| e.to_string())?) as Box<dyn Write + Send>,
                BufReader::new(Box::new(file) as Box<dyn std::io::Read + Send>),
            )
        };
        Ok(Self { writer, reader })
    }

    /// 发送 hello：把这条连接登记为「一个会话」（session 策略据此决定是否停内核）。
    pub fn hello(&mut self) -> Result<(), String> {
        self.call(&Request::Hello).map(|_| ())
    }

    /// 发送类型化请求并解析成类型化响应。
    ///
    /// 失败（`ok=false`）统一转成 `"<code>: <message>"`；协议版本不一致时给出
    /// 明确的升级指引，而不是一句「解析失败」。
    pub fn call(&mut self, req: &Request) -> Result<Response, String> {
        let wire = serde_json::to_value(WireRequest::new(req.clone()))
            .map_err(|e| format!("序列化请求失败: {e}"))?;
        self.send(wire)?;
        let value = self.read_reply()?;
        let resp: Response = serde_json::from_value(value).map_err(|e| {
            format!("响应不是协议 v2（服务端可能还是旧版，请把客户端与服务一起升级）: {e}")
        })?;
        into_result(resp)
    }

    fn send(&mut self, body: Value) -> Result<(), String> {
        writeln!(self.writer, "{body}").map_err(|e| format!("发送请求失败: {e}"))?;
        self.writer.flush().ok();
        Ok(())
    }

    fn read_reply(&mut self) -> Result<Value, String> {
        let mut line = String::new();
        self.reader
            .read_line(&mut line)
            .map_err(|e| format!("读取响应失败: {e}"))?;
        if line.trim().is_empty() {
            return Err("服务返回空响应".to_string());
        }
        serde_json::from_str(line.trim()).map_err(|e| format!("解析响应失败: {e}"))
    }
}

/// 把响应信封变成 `Result`：`ok=false` 一律错误，`code` 保留在消息前缀里。
///
/// 公开给**自带传输层**的客户端（GUI 要保持一条常驻 session，不复用 [`Client`]），
/// 保证两边的错误文案与协议校验完全一致。
pub fn into_result(resp: Response) -> Result<Response, String> {
    if resp.ok {
        // 老服务（v1）的成功响应没有 `proto`：不能当成 v2 结果继续用。
        if resp.proto != Some(PROTO) {
            return Err(
                "服务响应不是协议 v2（服务端可能还是旧版，请把客户端与服务一起升级）".to_string(),
            );
        }
        return Ok(resp);
    }
    let code = resp.code.unwrap_or(ErrorCode::Unknown);
    let message = resp.message.unwrap_or_else(|| "服务返回失败".to_string());
    if code == ErrorCode::ProtoMismatch {
        return Err(format!(
            "{}: {message}（客户端与服务 IPC 协议不一致，请升级到同一版本）",
            code.as_str()
        ));
    }
    Err(format!("{}: {message}", code.as_str()))
}

/// Windows：打开命名管道，遇到 `ERROR_PIPE_BUSY (231)` 时等实例空出来再试。
///
/// 服务端现在会预建多个实例（见 `exvlan-service` 的 `pipe_loop`），但并发高峰
/// （GUI 会话 + gateway 多端点 + CLI）仍可能瞬时占满；客户端必须自带重试，
/// 否则表现就是「莫名其妙连不上服务」。
#[cfg(windows)]
fn open_pipe_with_retry(addr: &std::path::Path) -> Result<std::fs::File, String> {
    use windows_sys::Win32::System::Pipes::WaitNamedPipeW;

    let name: Vec<u16> = addr
        .to_string_lossy()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut last = String::new();
    for attempt in 0..4u32 {
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(addr)
        {
            Ok(file) => return Ok(file),
            Err(err) => {
                let busy = err.raw_os_error() == Some(231); // ERROR_PIPE_BUSY
                last = err.to_string();
                if !busy {
                    break;
                }
                // 等一个实例空出来（最多 600ms），然后重试。
                unsafe {
                    WaitNamedPipeW(name.as_ptr(), 600);
                }
                std::thread::sleep(std::time::Duration::from_millis(100 * u64::from(attempt)));
            }
        }
    }
    Err(format!("连接服务管道失败: {last}"))
}

/// 一次性请求（连接 → 请求 → 关闭）。适合 CLI/doctor 这类短命令。
///
/// 注意：服务把「连接」当会话（`session` 策略下用它判断内核该不该停），
/// 所以这条短连接的生命周期就是本次调用。
pub fn call(req: &Request) -> Result<Response, String> {
    let mut client = Client::connect()?;
    client.call(req)
}

/// 诊断用：把响应负载转成 JSON（`doctor` 的 evidence 要原样带出去）。
pub fn data_json(resp: &Response) -> Value {
    resp.data
        .as_ref()
        .and_then(|d| serde_json::to_value(d).ok())
        .unwrap_or(Value::Null)
}
