//! 错误处理模块
//!
//! 提供 RPC 客户端的错误类型定义。

use std::io;
use std::net::AddrParseError;
use thiserror::Error;

/// RPC 客户端错误类型
///
/// # 示例
/// ```rust
/// use exvlan_rpc::error::RpcError;
///
/// fn example() -> Result<(), RpcError> {
///     // 处理错误...
///     Ok(())
/// }
/// ```
#[derive(Error, Debug)]
pub enum RpcError {
    /// 连接错误
    #[error("Connection error: {0}")]
    ConnectionError(String),

    /// RPC 响应错误
    #[error("RPC error: {0}")]
    RpcResponseError(String),

    /// 超时错误
    #[error("Timeout error")]
    TimeoutError,

    /// 序列化错误
    #[error("Serialization error: {0}")]
    SerializationError(String),

    /// 地址解析错误
    #[error("Address parse error: {0}")]
    AddrParse(#[from] AddrParseError),

    /// IO 错误
    #[error("IO error: {0}")]
    Io(#[from] io::Error),
}

impl RpcError {
    pub fn connection_error(msg: impl Into<String>) -> Self {
        Self::ConnectionError(msg.into())
    }

    pub fn rpc_response_error(msg: impl Into<String>) -> Self {
        Self::RpcResponseError(msg.into())
    }

    pub fn timeout_error() -> Self {
        Self::TimeoutError
    }

    pub fn serialization_error(msg: impl Into<String>) -> Self {
        Self::SerializationError(msg.into())
    }

    pub fn is_connection_error(&self) -> bool {
        matches!(self, Self::ConnectionError(_))
    }

    pub fn is_timeout_error(&self) -> bool {
        matches!(self, Self::TimeoutError)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rpc_error_connection_error() {
        let error = RpcError::ConnectionError("Connection failed".to_string());
        assert!(
            error
                .to_string()
                .contains("Connection error: Connection failed")
        );
        assert!(error.is_connection_error());
        assert!(!error.is_timeout_error());
    }

    #[test]
    fn test_rpc_error_connection_error_from_string() {
        let error = RpcError::connection_error("Connection refused");
        assert!(error.to_string().contains("Connection refused"));
    }

    #[test]
    fn test_rpc_error_connection_error_empty() {
        let error = RpcError::ConnectionError(String::new());
        assert!(error.to_string().contains("Connection error: "));
    }

    #[test]
    fn test_rpc_error_rpc_response_error() {
        let error = RpcError::RpcResponseError("Invalid response".to_string());
        assert!(error.to_string().contains("RPC error: Invalid response"));
        assert!(!error.is_connection_error());
        assert!(!error.is_timeout_error());
    }

    #[test]
    fn test_rpc_error_rpc_response_error_from_string() {
        let error = RpcError::rpc_response_error("Service unavailable");
        assert!(error.to_string().contains("Service unavailable"));
    }

    #[test]
    fn test_rpc_error_timeout_error() {
        let error = RpcError::TimeoutError;
        assert_eq!(error.to_string(), "Timeout error");
        assert!(error.is_timeout_error());
        assert!(!error.is_connection_error());
    }

    #[test]
    fn test_rpc_error_timeout_error_helper() {
        let error = RpcError::timeout_error();
        assert!(error.is_timeout_error());
    }

    #[test]
    fn test_rpc_error_serialization_error() {
        let error = RpcError::SerializationError("JSON parse error".to_string());
        assert!(
            error
                .to_string()
                .contains("Serialization error: JSON parse error")
        );
        assert!(!error.is_connection_error());
        assert!(!error.is_timeout_error());
    }

    #[test]
    fn test_rpc_error_serialization_error_from_string() {
        let error = RpcError::serialization_error("Invalid UTF-8");
        assert!(error.to_string().contains("Invalid UTF-8"));
    }

    #[test]
    fn test_rpc_error_addr_parse_error() {
        let addr_parse_error: Result<std::net::SocketAddr, AddrParseError> =
            "invalid-address".parse();
        let error = RpcError::from(addr_parse_error.unwrap_err());
        assert!(error.to_string().contains("Address parse error"));
        assert!(!error.is_connection_error());
        assert!(!error.is_timeout_error());
    }

    #[test]
    fn test_rpc_error_addr_parse_error_valid_ip() {
        let valid_ip: Result<std::net::IpAddr, AddrParseError> = "192.168.1.1".parse();
        assert!(valid_ip.is_ok());
    }

    #[test]
    fn test_rpc_error_addr_parse_error_invalid_port() {
        let addr_parse_error: Result<std::net::SocketAddr, AddrParseError> =
            "192.168.1.1:99999".parse();
        assert!(addr_parse_error.is_err());
    }

    #[test]
    fn test_rpc_error_io_error() {
        let io_error = io::Error::new(io::ErrorKind::ConnectionRefused, "Connection refused");
        let error = RpcError::from(io_error);
        assert!(error.to_string().contains("IO error: Connection refused"));
        assert!(!error.is_connection_error());
        assert!(!error.is_timeout_error());
    }

    #[test]
    fn test_rpc_error_io_error_various_kinds() {
        let kinds = [
            (io::ErrorKind::NotFound, "File not found"),
            (io::ErrorKind::PermissionDenied, "Permission denied"),
            (io::ErrorKind::ConnectionRefused, "Connection refused"),
            (io::ErrorKind::ConnectionReset, "Connection reset"),
            (io::ErrorKind::BrokenPipe, "Broken pipe"),
            (io::ErrorKind::WouldBlock, "Would block"),
            (io::ErrorKind::TimedOut, "Timed out"),
            (io::ErrorKind::Interrupted, "Interrupted"),
            (io::ErrorKind::AlreadyExists, "Already exists"),
            (io::ErrorKind::AddrInUse, "Address in use"),
            (io::ErrorKind::AddrNotAvailable, "Address not available"),
        ];

        for (kind, msg) in kinds {
            let io_err = io::Error::new(kind, msg);
            let error = RpcError::from(io_err);
            let error_str = error.to_string();
            assert!(
                error_str.contains("IO error:"),
                "Failed for kind: {:?}",
                kind
            );
        }
    }

    #[test]
    fn test_rpc_error_io_error_without_message() {
        let io_error = io::Error::new(io::ErrorKind::NotFound, "");
        let error = RpcError::from(io_error);
        assert!(error.to_string().contains("IO error:"));
    }

    #[test]
    fn test_rpc_error_debug_format() {
        let error = RpcError::ConnectionError("test".to_string());
        let debug_str = format!("{:?}", error);
        assert!(debug_str.contains("ConnectionError"));
    }

    #[test]
    fn test_rpc_error_conversion_from_io_error() {
        let io_error = io::Error::last_os_error();
        let _error: RpcError = RpcError::from(io_error);
    }

    #[test]
    fn test_rpc_error_conversion_from_addr_parse_error() {
        let addr_error = "invalid".parse::<std::net::SocketAddr>().unwrap_err();
        let _error: RpcError = RpcError::from(addr_error);
    }

    #[test]
    fn test_rpc_error_default_error_kinds() {
        assert!(RpcError::timeout_error().is_timeout_error());
        assert!(RpcError::connection_error("test").is_connection_error());
    }
}
