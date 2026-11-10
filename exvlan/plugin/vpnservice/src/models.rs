#![allow(dead_code)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PingRequest {
    pub value: Option<String>,
}

/// 用系统安装器打开已下载的 APK（走 FileProvider，无需存储权限）。
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallApkRequest {
    pub path: String,
}

/// 用系统分享面板分享一个应用私有目录里的文件（走 FileProvider，无需存储权限）。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareFileRequest {
    pub path: String,
    #[serde(default)]
    pub mime: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
}

/// 保存到系统「下载」目录的结果（展示路径，如 `Download/Exvlan/xxx.zip`）。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveToDownloadsResponse {
    #[serde(default)]
    pub path: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PingResponse {
    pub value: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoidRequest {}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartVpnRequest {
    pub ipv4_addr: Option<String>,
    pub routes: Option<Vec<String>>,
    pub dns: Option<String>,
    pub disallowed_applications: Option<Vec<String>>,
    pub mtu: Option<u32>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub error_msg: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VpnStatus {
    pub running: bool,
    pub ipv4_addr: Option<String>,
    pub routes: Option<Vec<String>>,
    pub dns: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VpnTileActionResponse {
    pub action: Option<String>,
}
