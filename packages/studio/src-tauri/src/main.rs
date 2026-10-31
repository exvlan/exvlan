#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() -> std::process::ExitCode {
    // 提权子命令（安装内核 / 安装服务 / 启停服务）先于 GUI 处理。
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(code) = app_lib::run_elevated_cli(&args) {
        return code;
    }
    app_lib::run_gui()
}
