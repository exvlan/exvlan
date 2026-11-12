//! 特权服务 IPC 客户端。
//!
//! 协议：unix socket（macOS/Linux）或 Windows 命名管道，**行式 JSON**：
//!
//! ```text
//! → {"proto":1,"op":"kernel.ensure"}
//! ← {"ok":true,"mode":"service","kernel":{...}}
//! ```
//!
//! 鉴权由操作系统负责（socket `0600` + 属主 / 命名管道 SDDL），这里不做自建鉴权。

use std::io::{BufRead as _, BufReader, Write};

use serde_json::{Value, json};

use crate::paths;

/// 一条持久会话。
///
/// 「normal」策略下服务的生命周期靠它：断开即视为会话结束。
pub struct Client {
    writer: Box<dyn Write + Send>,
    reader: BufReader<Box<dyn std::io::Read + Send>>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("service-client::Client")
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

        #[cfg(windows)]
        let stream = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&addr)
            .map_err(|e| format!("连接服务管道失败: {e}"))?;
        #[cfg(windows)]
        let _ = stream;

        #[cfg(unix)]
        let (writer, reader) = {
            (
                Box::new(stream.try_clone().map_err(|e| e.to_string())?)
                    as Box<dyn Write + Send>,
                BufReader::new(Box::new(stream) as Box<dyn std::io::Read + Send>),
            )
        };
        #[cfg(windows)]
        let (writer, reader) = {
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&addr)
                .map_err(|e| format!("连接服务管道失败: {e}"))?;
            (
                Box::new(file.try_clone().map_err(|e| e.to_string())?)
                    as Box<dyn Write + Send>,
                BufReader::new(Box::new(file) as Box<dyn std::io::Read + Send>),
            )
        };
        Ok(Self { writer, reader })
    }

    /// 发送 hello：把这条连接登记为「一个会话」（normal 策略据此决定是否停内核）。
    pub fn hello(&mut self) -> Result<(), String> {
        self.send(json!({"proto":1,"op":"hello"}))?;
        self.read_reply().map(|_| ())
    }

    /// 发一个请求并等待响应。
    pub fn request(&mut self, op: &str, extra: Value) -> Result<Value, String> {
        let mut body = json!({ "proto": 1, "op": op });
        if let (Some(dst), Some(src)) = (body.as_object_mut(), extra.as_object()) {
            for (k, v) in src {
                dst.insert(k.clone(), v.clone());
            }
        }
        self.send(body)?;
        self.read_reply()
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
        let resp: Value =
            serde_json::from_str(line.trim()).map_err(|e| format!("解析响应失败: {e}"))?;
        if resp.get("ok").and_then(Value::as_bool).unwrap_or(false) {
            Ok(resp)
        } else {
            let code = resp.get("code").and_then(Value::as_str).unwrap_or("error");
            let msg = resp
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("服务返回失败");
            Err(format!("{code}: {msg}"))
        }
    }
}

/// 一次性请求（连接 → 请求 → 关闭）。适合 CLI/doctor 这类短命令。
pub fn request(op: &str, extra: Value) -> Result<Value, String> {
    let mut client = Client::connect()?;
    client.request(op, extra)
}
