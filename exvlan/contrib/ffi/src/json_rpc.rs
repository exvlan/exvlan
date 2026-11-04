use std::{
    ffi::{CString, c_char, c_int},
    sync::Arc,
};

use crate::{
    config_server::in_config_server_callback,
    error::set_error_msg,
    state::ffi_context,
    strings::{c_str_to_string, optional_c_str_to_string},
};

/// # Safety
/// See `crate::call_json_rpc`.
pub(crate) unsafe fn call_json_rpc(
    service_name: *const c_char,
    method_name: *const c_char,
    domain_name: *const c_char,
    payload_json: *const c_char,
    out_response_json: *mut *const c_char,
) -> c_int {
    // panic 不能穿过 `extern "C"` 边界（未定义行为 / 直接 abort）。移动端过去
    // 一次 panic 就是整个应用闪退，且用户拿不到任何现场；这里兜住，转成一条
    // 普通 RPC 错误返回给 GUI（配合 `[profile.release] panic = "unwind"`）。
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        call_json_rpc_inner(
            service_name,
            method_name,
            domain_name,
            payload_json,
            out_response_json,
        )
    }));

    match result {
        Ok(code) => code,
        Err(payload) => {
            let detail = payload
                .downcast_ref::<&str>()
                .map(|s| (*s).to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "<non-string panic payload>".to_string());
            set_error_msg(&format!("panic in call_json_rpc: {detail}"));
            -1
        }
    }
}

unsafe fn call_json_rpc_inner(
    service_name: *const c_char,
    method_name: *const c_char,
    domain_name: *const c_char,
    payload_json: *const c_char,
    out_response_json: *mut *const c_char,
) -> c_int {
    if out_response_json.is_null() {
        set_error_msg("out_response_json is null");
        return -1;
    }
    unsafe {
        *out_response_json = std::ptr::null();
    }

    if in_config_server_callback() {
        set_error_msg("cannot call JSON RPC from config server callback");
        return -1;
    }

    let service_name = match unsafe { c_str_to_string(service_name, "service_name") } {
        Ok(value) => value,
        Err(err) => {
            set_error_msg(&err);
            return -1;
        }
    };
    let method_name = match unsafe { c_str_to_string(method_name, "method_name") } {
        Ok(value) => value,
        Err(err) => {
            set_error_msg(&err);
            return -1;
        }
    };
    let domain_name = match unsafe { optional_c_str_to_string(domain_name, "domain_name") } {
        Ok(value) => value,
        Err(err) => {
            set_error_msg(&err);
            return -1;
        }
    };
    let payload_json = match unsafe { c_str_to_string(payload_json, "payload_json") } {
        Ok(value) => value,
        Err(err) => {
            set_error_msg(&err);
            return -1;
        }
    };
    let payload = match serde_json::from_str::<serde_json::Value>(&payload_json) {
        Ok(value) => value,
        Err(err) => {
            set_error_msg(&format!("failed to parse payload_json: {}", err));
            return -1;
        }
    };

    let response = match ffi_context().runtime.block_on(async {
        // 实例管理面（GUI 移动端）：分发到进程级 ProcessManagementRpc。
        if service_name == "api.manage.WebClientService" {
            exvlan_core::management::call_web_client_json_rpc(
                &ffi_context().web_client_rpc,
                &method_name,
                payload,
            )
            .await
        } else {
            exvlan_core::management::call_management_json_rpc(
                &ffi_context().manager,
                Arc::new(exvlan::rpc_service::logger::NativeLoggerControl),
                &service_name,
                &method_name,
                domain_name.as_deref(),
                payload,
            )
            .await
        }
    }) {
        Ok(value) => value,
        Err(err) => {
            set_error_msg(&format!("RPC Error: {}", err));
            return -1;
        }
    };
    let response_json = match serde_json::to_string(&response) {
        Ok(value) => value,
        Err(err) => {
            set_error_msg(&format!("failed to serialize RPC response: {}", err));
            return -1;
        }
    };
    let response_json = match CString::new(response_json) {
        Ok(value) => value,
        Err(err) => {
            set_error_msg(&format!("failed to allocate RPC response: {}", err));
            return -1;
        }
    };

    unsafe {
        *out_response_json = response_json.into_raw();
    }
    0
}

#[cfg(test)]
mod mobile_sequence_tests {
    //! 复现移动端首页的真实调用序列。
    //!
    //! 20708/20709 在「进入首页」时闪退：移动端 GUI 会依次调用
    //! `list_network_instance` / `collect_network_info` / `list_network_instance_meta`
    //! 等管理 RPC。这个用例在 Linux 上用**同一套内核代码**跑一遍，
    //! `catch_unwind` 能直接指出是哪个方法 panic（测试是 unwind，真机是 abort）。
    use super::*;
    use exvlan_core::management::{ProcessManagementRpc, call_web_client_json_rpc};
    use exvlan_proto::api::manage::{
        CollectNetworkInfoRequest, GetNetworkInstanceConfigRequest, ListNetworkInstanceMetaRequest,
        ListNetworkInstanceRequest, NetworkConfig, RunNetworkInstanceRequest,
        ValidateConfigRequest,
    };

    fn call(req: serde_json::Value, method: &str) -> Result<serde_json::Value, String> {
        let ctx = ffi_context();
        let rpc: &ProcessManagementRpc<_> = &ctx.web_client_rpc;
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            ctx.runtime
                .block_on(call_web_client_json_rpc(rpc, method, req))
        }));
        match outcome {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => Err(e.to_string()),
            Err(_) => Err("PANIC".to_string()),
        }
    }

    #[test]
    fn mobile_home_sequence_does_not_panic() {
        let inst = uuid::Uuid::new_v4();
        let cfg = NetworkConfig {
            instance_id: Some(inst.to_string()),
            ..Default::default()
        };

        let cases: Vec<(&str, serde_json::Value)> = vec![
            (
                "list_network_instance",
                serde_json::to_value(ListNetworkInstanceRequest {}).unwrap(),
            ),
            (
                "collect_network_info",
                serde_json::to_value(CollectNetworkInfoRequest { inst_ids: vec![] }).unwrap(),
            ),
            (
                "list_network_instance_meta",
                serde_json::to_value(ListNetworkInstanceMetaRequest {
                    inst_ids: vec![inst.into()],
                })
                .unwrap(),
            ),
            (
                "get_network_instance_config",
                serde_json::to_value(GetNetworkInstanceConfigRequest {
                    inst_id: Some(inst.into()),
                })
                .unwrap(),
            ),
            (
                "validate_config",
                serde_json::to_value(ValidateConfigRequest {
                    config: Some(cfg.clone()),
                })
                .unwrap(),
            ),
            (
                "run_network_instance",
                serde_json::to_value(RunNetworkInstanceRequest {
                    inst_id: None,
                    config: Some(cfg),
                    overwrite: true,
                    source: 1,
                })
                .unwrap(),
            ),
        ];

        for (method, payload) in cases {
            match call(payload, method) {
                Ok(_) => eprintln!("[ok] {method}"),
                Err(err) if err == "PANIC" => panic!("{method} panicked（真机上就是闪退）"),
                Err(err) => eprintln!("[err] {method}: {err}（业务错误可接受）"),
            }
        }
    }
}
