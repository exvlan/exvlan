#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() -> std::process::ExitCode {
    // 内核以独立二进制运行（`exvlan --daemon`），GUI 二进制不再有 CLI 入口。
    app_lib::run_gui()
}
