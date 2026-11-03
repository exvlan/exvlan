//! 日志配置类型（本地定义，避免 `exvlan-rpc` 为日志拉入内核 crate）。
//!
//! 内核侧有自己的 `config::toml::{FileLoggerConfig, LoggingConfigLoader}`；
//! GUI/移动端只需要「console level + 文件滚动参数」，这里给一份最小等价物，
//! 由调用方（studio）自行组装。

/// 单文件日志选项。
#[derive(Debug, Clone, Default)]
pub struct FileLoggerConfig {
    pub level: Option<String>,
    pub file: Option<String>,
    pub dir: Option<String>,
    pub size_mb: Option<u64>,
    pub count: Option<usize>,
}

/// 控制台日志选项。
#[derive(Debug, Clone, Default)]
pub struct ConsoleLoggerConfig {
    pub level: Option<String>,
}

/// 供 [`super::init`] 使用的配置来源。
pub trait LoggingConfigLoader {
    fn get_console_logger_config(&self) -> ConsoleLoggerConfig;
    fn get_file_logger_config(&self) -> FileLoggerConfig;
}

/// 便捷实现：直接给一对配置。
#[derive(Debug, Clone, Default)]
pub struct SimpleLoggingConfig {
    pub console: ConsoleLoggerConfig,
    pub file: FileLoggerConfig,
}

impl SimpleLoggingConfig {
    pub fn new(console: ConsoleLoggerConfig, file: FileLoggerConfig) -> Self {
        Self { console, file }
    }
}

impl LoggingConfigLoader for SimpleLoggingConfig {
    fn get_console_logger_config(&self) -> ConsoleLoggerConfig {
        self.console.clone()
    }

    fn get_file_logger_config(&self) -> FileLoggerConfig {
        self.file.clone()
    }
}

impl<T: LoggingConfigLoader + ?Sized> LoggingConfigLoader for &T {
    fn get_console_logger_config(&self) -> ConsoleLoggerConfig {
        (**self).get_console_logger_config()
    }

    fn get_file_logger_config(&self) -> FileLoggerConfig {
        (**self).get_file_logger_config()
    }
}
