// 经 gateway 的远端客户端（R2：只读）。
//
// 与 `GUIRemoteClient`（本机 / 直连内核 RPC）并列：这里全部走对端的 HTTP gateway，
// 鉴权用对端签发的 Bearer token（token 只在 Rust 侧，见 `src-tauri/src/gateway_remote.rs`）。
//
// 只读约束是**双重**的：
//   1. UI：`config_permission` 强制 READ_ONLY|NO_DELETE → 编辑/删除入口自动消失；
//   2. 这里：写方法直接抛错（不静默失败），避免「看起来能点、点了没反应」。
import { Api, type NetworkTypes, Utils } from "@exvlan/std";
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
  config?: NetworkTypes.NetworkConfig;
  info?: Api.CollectNetworkInfoResponse;
  meta?: Api.NetworkMeta | null;
  saved?: { enabled: boolean; text: string } | null;
}

/** 只读目标：权限位固定为「只读 + 不可删」。 */
const READ_ONLY_PERMISSION =
  Api.ConfigFilePermission.READ_ONLY | Api.ConfigFilePermission.NO_DELETE;

const READ_ONLY_HINT = "这是「经 gateway」的只读目标：需要修改请在对端本机操作。";

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
          metas[id] = {
            network_name: snapshot.meta?.network_name || id.slice(0, 8),
            config_permission: READ_ONLY_PERMISSION,
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

  // -- 以下为只读目标明确不支持的操作 --------------------------------------

  async get_vpn_portal_info(_inst_id: string): Promise<NetworkTypes.VpnPortalInfo | undefined> {
    return undefined;
  }

  async validate_config(_config: NetworkTypes.NetworkConfig): Promise<Api.ValidateConfigResponse> {
    throw new Error(READ_ONLY_HINT);
  }

  async run_network(_config: NetworkTypes.NetworkConfig, _save: boolean): Promise<undefined> {
    throw new Error(READ_ONLY_HINT);
  }

  async add_vpn_portal_client(
    _inst_id: string,
    _client: { name: string; virtual_ip: string; groups: string[] },
  ): Promise<undefined> {
    throw new Error(READ_ONLY_HINT);
  }

  async remove_vpn_portal_client(_inst_id: string, _name: string): Promise<undefined> {
    throw new Error(READ_ONLY_HINT);
  }

  async clear_vpn_portal_clients(_inst_id: string): Promise<undefined> {
    throw new Error(READ_ONLY_HINT);
  }

  async delete_network(_inst_id: string): Promise<undefined> {
    throw new Error(READ_ONLY_HINT);
  }

  async update_network_instance_state(_inst_id: string, _disabled: boolean): Promise<undefined> {
    throw new Error(READ_ONLY_HINT);
  }

  async save_config(_config: NetworkTypes.NetworkConfig): Promise<undefined> {
    throw new Error(READ_ONLY_HINT);
  }

  async generate_config(_config: NetworkTypes.NetworkConfig): Promise<Api.GenerateConfigResponse> {
    throw new Error(READ_ONLY_HINT);
  }

  async parse_config(_toml_config: string): Promise<Api.ParseConfigResponse> {
    throw new Error(READ_ONLY_HINT);
  }
}
