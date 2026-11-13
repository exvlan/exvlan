// 经 gateway 的远端客户端。
//
// 与 `GUIRemoteClient`（本机 / 直连内核 RPC）并列：这里全部走对端的 HTTP gateway，
// 鉴权用对端签发的 Bearer token（token 只在 Rust 侧，见 `src-tauri/src/gateway_remote.rs`）。
//
// 权限由**对端 token** 决定：配对时 role=readonly 只能读；role=config/admin 才有
// `kernel:write`。UI 侧按 `writable` 决定是否显示编辑/删除入口；真正越权时对端返回
// 403「token 缺少 scope」，这里原样抛给用户（提示改用更强角色重新配对）。
import { Api, NetworkTypes, Utils } from "@exvlan/std";
import { invoke } from "@tauri-apps/api/core";

/** 与 Rust 侧 `gateway_overview` 的返回对应。 */
interface GatewayOverview {
  running: string[];
  disabled: string[];
  instances: Array<{ id: string; config?: NetworkTypes.NetworkConfig }>;
  saved: Array<{ id: string; enabled: boolean; bytes: number; modified?: string }>;
}

interface GatewayInstance {
  id: string;
  running: boolean;
  /** 本机 token 是否带 kernel:write（由 Rust 侧按 scopes 计算）。 */
  writable?: boolean;
  config?: NetworkTypes.NetworkConfig;
  info?: Api.CollectNetworkInfoResponse;
  meta?: Api.NetworkMeta | null;
  saved?: { enabled: boolean; text: string } | null;
}

/** 只读目标：权限位固定为「只读 + 不可删」。 */
const READ_ONLY_PERMISSION =
  Api.ConfigFilePermission.READ_ONLY | Api.ConfigFilePermission.NO_DELETE;

const READ_ONLY_HINT =
  "当前 token 是只读的（配对时选了「只读」）。要修改请在对端重新生成配对码并选「可改配置」或「全权」。";

export class GatewayRemoteClient implements Api.RemoteClient {
  constructor(private readonly targetId: string) {}

  private async overview(): Promise<GatewayOverview> {
    return await invoke<GatewayOverview>("gateway_overview", { targetId: this.targetId });
  }

  private async snapshot(instId: string): Promise<GatewayInstance> {
    return await invoke<GatewayInstance>("gateway_instance", {
      targetId: this.targetId,
      instanceId: instId,
    });
  }

  async list_network_instance_ids(): Promise<Api.ListNetworkInstanceIdResponse> {
    const overview = await this.overview();
    return {
      running_inst_ids: (overview.running ?? []).map((id) => Utils.StrToUuid(id)),
      disabled_inst_ids: (overview.disabled ?? []).map((id) => Utils.StrToUuid(id)),
    };
  }

  async get_network_config(inst_id: string): Promise<NetworkTypes.NetworkConfig> {
    const snapshot = await this.snapshot(inst_id);
    if (!snapshot.config) {
      throw new Error(`对端没有实例 ${inst_id} 的运行配置（可能已停止）`);
    }
    return snapshot.config;
  }

  async get_network_info(
    inst_id: string,
  ): Promise<NetworkTypes.NetworkInstanceRunningInfo | undefined> {
    const snapshot = await this.snapshot(inst_id);
    // 网关转发的是内核 `collect_network_info` 的原样 JSON：`{info:{map:{<id>:...}}}`。
    return snapshot.info?.info?.map?.[inst_id];
  }

  async get_network_metas(instance_ids: string[]): Promise<Api.GetNetworkMetasResponse> {
    const metas: Record<string, Api.NetworkMeta> = {};
    await Promise.all(
      instance_ids.map(async (id) => {
        try {
          const snapshot = await this.snapshot(id);
          // 有 kernel:write 才沿用对端的权限位，否则强制只读（UI 直接隐藏入口）。
          metas[id] = {
            network_name: snapshot.meta?.network_name || id.slice(0, 8),
            config_permission: snapshot.writable
              ? (snapshot.meta?.config_permission ?? 0)
              : READ_ONLY_PERMISSION,
          };
        } catch (e) {
          // 单个实例读不到不该让整页空白：记一条 warn，名字退回短 id。
          console.warn("gateway meta failed", id, e);
          metas[id] = { network_name: id.slice(0, 8), config_permission: READ_ONLY_PERMISSION };
        }
      }),
    );
    return { metas };
  }

  // -- 写操作（需要 token 带 kernel:write，否则对端 403） --------------------

  async run_network(config: NetworkTypes.NetworkConfig, _save: boolean): Promise<undefined> {
    // 网关侧 run 一定会持久化到对端配置目录（`save` 本地才有意义）。
    await invoke("gateway_run_instance", {
      targetId: this.targetId,
      config: NetworkTypes.toBackendNetworkConfig(config),
    });
  }

  async update_network_instance_state(inst_id: string, disabled: boolean): Promise<undefined> {
    await invoke("gateway_update_state", {
      targetId: this.targetId,
      instanceId: inst_id,
      disabled,
    });
  }

  async delete_network(inst_id: string): Promise<undefined> {
    await invoke("gateway_delete_instance", { targetId: this.targetId, instanceId: inst_id });
  }

  /** 保存配置：网关侧就是「按配置重跑一次」（会持久化）。 */
  async save_config(config: NetworkTypes.NetworkConfig): Promise<undefined> {
    await this.run_network(config, true);
  }

  async validate_config(config: NetworkTypes.NetworkConfig): Promise<Api.ValidateConfigResponse> {
    const result = await invoke<{ toml_config?: string }>("gateway_validate", {
      targetId: this.targetId,
      config: NetworkTypes.toBackendNetworkConfig(config),
    });
    return { toml_config: result.toml_config ?? "" };
  }

  async get_vpn_portal_info(_inst_id: string): Promise<NetworkTypes.VpnPortalInfo | undefined> {
    // 门户信息来自内核 info（只读路径已覆盖），这里不额外发请求。
    return undefined;
  }

  async add_vpn_portal_client(
    _inst_id: string,
    _client: { name: string; virtual_ip: string; groups: string[] },
  ): Promise<undefined> {
    throw new Error("经 gateway 暂不支持编辑门户客户端（请在对端本机操作）");
  }

  async remove_vpn_portal_client(_inst_id: string, _name: string): Promise<undefined> {
    throw new Error("经 gateway 暂不支持编辑门户客户端（请在对端本机操作）");
  }

  async clear_vpn_portal_clients(_inst_id: string): Promise<undefined> {
    throw new Error("经 gateway 暂不支持编辑门户客户端（请在对端本机操作）");
  }

  async generate_config(_config: NetworkTypes.NetworkConfig): Promise<Api.GenerateConfigResponse> {
    throw new Error(READ_ONLY_HINT);
  }

  async parse_config(_toml_config: string): Promise<Api.ParseConfigResponse> {
    throw new Error(READ_ONLY_HINT);
  }
}
