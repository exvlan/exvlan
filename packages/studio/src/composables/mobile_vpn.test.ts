import { beforeEach, describe, expect, it, vi } from "vite-plus/test";

const mocks = vi.hoisted(() => {
  const listeners = new Map<string, (payload: unknown) => Promise<void>>();
  const configs = new Map<string, Record<string, unknown>>();
  const networkInfo = new Map<string, unknown>();

  return {
    listeners,
    configs,
    networkInfo,
    addPluginListener: vi.fn(
      async (_plugin: string, event: string, listener: (payload: unknown) => Promise<void>) => {
        listeners.set(event, listener);
      },
    ),
    collectNetworkInfo: vi.fn(async (instanceId: string) => ({
      info: { map: { [instanceId]: networkInfo.get(instanceId) } },
    })),
    consumeVpnTileAction: vi.fn(async () => ({})),
    getConfig: vi.fn(async (instanceId: string) => configs.get(instanceId)),
    getVpnStatus: vi.fn<() => Promise<Record<string, unknown>>>(async () => ({ running: false })),
    logVpnRoutes: vi.fn(async () => undefined),
    listNetworkInstanceIds: vi.fn<() => Promise<{ running_inst_ids: unknown[] }>>(async () => ({
      running_inst_ids: [],
    })),
    // 与 Node 测试环境一致：没有 Tauri 运行时时 osType() 会抛 "window is not defined"。
    osType: vi.fn((): string => {
      throw new ReferenceError("window is not defined");
    }),
    prepareVpn: vi.fn(async () => ({ granted: true })),
    reportFrontendError: vi.fn(async () => undefined),
    setTunFd: vi.fn(async () => undefined),
    startVpn: vi.fn(async () => {
      await listeners.get("vpn_service_start")?.({ fd: 1 });
      return {};
    }),
    stopVpn: vi.fn(async () => {
      await listeners.get("vpn_service_stop")?.({});
      return {};
    }),
  };
});

vi.mock("@tauri-apps/api/core", () => ({
  addPluginListener: mocks.addPluginListener,
}));

vi.mock("@exvlan/std", () => ({
  Utils: {
    UuidToStr: (value: unknown) => String(value),
    ipv4ToString: (address: { addr: string }) => address.addr,
    ipv6ToString: (address: { part1: number }) => `fd00:0:0:${address.part1}::`,
  },
}));

vi.mock("@tauri-apps/plugin-os", () => ({
  type: mocks.osType,
}));

vi.mock("tauri-plugin-vpnservice-api", () => ({
  consume_vpn_tile_action: mocks.consumeVpnTileAction,
  get_vpn_status: mocks.getVpnStatus,
  prepare_vpn: mocks.prepareVpn,
  start_vpn: mocks.startVpn,
  stop_vpn: mocks.stopVpn,
}));

vi.mock("./backend", () => ({
  collectNetworkInfo: mocks.collectNetworkInfo,
  getConfig: mocks.getConfig,
  listNetworkInstanceIds: mocks.listNetworkInstanceIds,
  logVpnRoutes: mocks.logVpnRoutes,
  reportFrontendError: mocks.reportFrontendError,
  setTunFd: mocks.setTunFd,
}));

function setConfig(instanceId: string, noTun = false, routes: string[] = []) {
  mocks.configs.set(instanceId, {
    no_tun: noTun,
    dhcp: false,
    enable_magic_dns: false,
    routes,
  });
}

function setReady(instanceId: string, ipv4: string) {
  mocks.networkInfo.set(instanceId, {
    my_node_info: {
      virtual_ipv4: {
        address: { addr: ipv4 },
        network_length: 24,
      },
    },
    routes: [],
  });
}

async function loadVpnModule() {
  const mobileVpn = await import("./mobile_vpn");
  await mobileVpn.initMobileVpnService();
  return mobileVpn;
}

beforeEach(() => {
  vi.useFakeTimers();
  vi.resetModules();
  mocks.listeners.clear();
  mocks.configs.clear();
  mocks.networkInfo.clear();
  mocks.addPluginListener.mockClear();
  mocks.collectNetworkInfo.mockClear();
  mocks.consumeVpnTileAction.mockReset();
  mocks.consumeVpnTileAction.mockResolvedValue({});
  mocks.getConfig.mockClear();
  mocks.getVpnStatus.mockReset();
  mocks.getVpnStatus.mockResolvedValue({ running: false });
  mocks.listNetworkInstanceIds.mockReset();
  mocks.listNetworkInstanceIds.mockResolvedValue({ running_inst_ids: [] });
  // 保持基线：默认没有 Tauri 运行时，osType() 抛错；需要 Android 的用例自行 mockReturnValue。
  mocks.osType.mockReset();
  mocks.osType.mockImplementation(() => {
    throw new ReferenceError("window is not defined");
  });
  mocks.prepareVpn.mockClear();
  mocks.reportFrontendError.mockClear();
  mocks.setTunFd.mockClear();
  mocks.startVpn.mockClear();
  mocks.stopVpn.mockClear();
});

describe("mobile VPN reconciliation ownership", () => {
  it("stops A before retrying an unavailable B, then starts B when it becomes ready", async () => {
    setConfig("A");
    setConfig("B");
    setReady("A", "10.0.0.1");
    const vpn = await loadVpnModule();

    await vpn.onNetworkInstanceChange("A");
    expect(mocks.startVpn).toHaveBeenCalledTimes(1);

    mocks.startVpn.mockClear();
    await vpn.onNetworkInstanceChange("B");

    expect(mocks.stopVpn).toHaveBeenCalledTimes(1);
    expect(mocks.startVpn).not.toHaveBeenCalled();

    setReady("B", "10.0.0.2");
    await vpn.onNetworkInstanceUpdate("B");

    expect(mocks.startVpn).toHaveBeenCalledTimes(1);
    expect(mocks.startVpn).toHaveBeenCalledWith(
      expect.objectContaining({ ipv4Addr: "10.0.0.2/24" }),
    );
  });

  it("stops the previous owner during pre-run even if the new instance never reaches post-run", async () => {
    setConfig("A");
    setConfig("B");
    setReady("A", "10.0.0.1");
    const vpn = await loadVpnModule();

    await vpn.onNetworkInstanceChange("A");
    mocks.stopVpn.mockClear();

    await vpn.prepareVpnService("B");

    expect(mocks.stopVpn).toHaveBeenCalledTimes(1);
  });

  it("preserves the VPN while retrying the same instance", async () => {
    setConfig("A");
    setReady("A", "10.0.0.1");
    const vpn = await loadVpnModule();

    await vpn.onNetworkInstanceChange("A");
    mocks.stopVpn.mockClear();
    mocks.networkInfo.delete("A");

    await vpn.onNetworkInstanceUpdate("A");

    expect(mocks.stopVpn).not.toHaveBeenCalled();
  });

  it("ignores an update from an instance that no longer owns the VPN", async () => {
    setConfig("A");
    setConfig("B");
    setReady("A", "10.0.0.1");
    const vpn = await loadVpnModule();

    await vpn.onNetworkInstanceChange("A");
    await vpn.onNetworkInstanceChange("B");
    mocks.collectNetworkInfo.mockClear();

    await vpn.onNetworkInstanceUpdate("A");

    expect(mocks.collectNetworkInfo).not.toHaveBeenCalled();
  });

  it("does not apply an in-flight result after the desired instance changes", async () => {
    setConfig("A");
    setConfig("B");
    setReady("A", "10.0.0.1");
    const vpn = await loadVpnModule();

    await vpn.onNetworkInstanceChange("A");
    mocks.startVpn.mockClear();
    mocks.stopVpn.mockClear();

    interface NetworkInfoResponse {
      info: { map: Record<string, unknown> };
    }
    let resolveNetworkInfo: (value: NetworkInfoResponse) => void = () => undefined;
    let markCollectStarted: () => void = () => undefined;
    const collectStarted = new Promise<void>((resolve) => {
      markCollectStarted = resolve;
    });
    mocks.collectNetworkInfo.mockImplementationOnce(
      async () =>
        await new Promise<NetworkInfoResponse>((resolve) => {
          resolveNetworkInfo = resolve;
          markCollectStarted();
        }),
    );

    const staleUpdate = vpn.onNetworkInstanceUpdate("A");
    await collectStarted;
    const switchToB = vpn.onNetworkInstanceChange("B");
    resolveNetworkInfo({
      info: {
        map: {
          A: {
            my_node_info: {
              virtual_ipv4: {
                address: { addr: "10.0.0.99" },
                network_length: 24,
              },
            },
            routes: [],
          },
        },
      },
    });

    await Promise.all([staleUpdate, switchToB]);

    expect(mocks.startVpn).not.toHaveBeenCalled();
    expect(mocks.stopVpn).toHaveBeenCalledTimes(1);
  });

  it("stops a native VPN with unknown ownership before retrying the selected instance", async () => {
    setConfig("A");
    mocks.getVpnStatus.mockResolvedValue({
      running: true,
      ipv4Addr: "10.0.0.1/24",
      routes: [],
    });
    mocks.listNetworkInstanceIds.mockResolvedValue({ running_inst_ids: ["A"] });
    const vpn = await loadVpnModule();

    await vpn.syncMobileVpnService();

    expect(mocks.stopVpn).toHaveBeenCalledTimes(1);
    expect(mocks.startVpn).not.toHaveBeenCalled();
  });
});

describe("mobile VPN excluded applications", () => {
  it("keeps the app's own package inside the tunnel so R2 traffic can reach the mesh", async () => {
    setConfig("A");
    setReady("A", "10.254.1.6");
    const vpn = await loadVpnModule();

    await vpn.onNetworkInstanceChange("A");

    expect(mocks.startVpn).toHaveBeenCalledTimes(1);
    const call = ((mocks.startVpn.mock.calls as unknown[][])[0] ?? [])[0] as
      | { disallowedApplications?: string[] }
      | undefined;
    expect(call?.disallowedApplications ?? []).not.toContain("era.exvlan.studio");
  });
});

describe("mobile VPN tile action delivery", () => {
  it("does not consume a pending action before a handler is ready", async () => {
    const vpn = await loadVpnModule();

    expect(await vpn.consumePendingMobileVpnTileAction()).toBe(false);
    expect(mocks.consumeVpnTileAction).not.toHaveBeenCalled();
  });

  it("consumes and dispatches a pending action once a handler is registered", async () => {
    const vpn = await loadVpnModule();
    const handler = vi.fn(async () => undefined);
    mocks.consumeVpnTileAction.mockResolvedValue({ action: "start" });
    vpn.setMobileVpnTileActionHandler(handler);

    expect(await vpn.consumePendingMobileVpnTileAction()).toBe(true);
    expect(handler).toHaveBeenCalledWith("start");
  });
});

describe("getRoutesForVpn", () => {
  it("includes the local subnet, peer addresses, proxy cidrs and magic dns", async () => {
    const vpn = await loadVpnModule();

    const routes = vpn.getRoutesForVpn(
      [
        {
          peer_id: 1,
          ipv4_addr: { address: { addr: "10.254.1.2" }, network_length: 27 },
          proxy_cidrs: [],
        },
        { peer_id: 2, ipv4_addr: "10.254.1.9", proxy_cidrs: ["192.168.5.7"] },
      ] as any,
      { routes: ["172.16.0.5"], enable_magic_dns: true } as any,
      "10.126.126.224",
      24,
    );

    expect(routes).toEqual([
      "10.126.126.0/24",
      "10.254.1.0/27",
      "10.254.1.9/32",
      "100.100.100.101/32",
      "172.16.0.5/32",
      "192.168.5.7/32",
    ]);
  });

  it("skips invalid entries instead of failing the whole VPN", async () => {
    const vpn = await loadVpnModule();

    const routes = vpn.getRoutesForVpn(
      [{ peer_id: 1, ipv4_addr: "not-an-ip", proxy_cidrs: ["10.0.0.1/99", ""] }] as any,
      { routes: [""] } as any,
    );

    expect(routes).toEqual([]);
  });

  it("adds IPv6 peer prefixes with their own network length", async () => {
    const vpn = await loadVpnModule();

    const routes = vpn.getRoutesForVpn(
      [
        {
          peer_id: 1,
          ipv4_addr: null,
          proxy_cidrs: [],
          ipv6_addr: { address: { part1: 1, part2: 2, part3: 3, part4: 4 }, network_length: 64 },
        },
      ] as any,
      {} as any,
    );

    expect(routes).toEqual(["fd00:0:0:1::/64"]);
  });
});

describe("Android global (default) route rejection", () => {
  function useAndroid() {
    mocks.osType.mockReturnValue("android");
  }

  it("rejects 0.0.0.0/0 on Android with an actionable error", async () => {
    const vpn = await loadVpnModule();

    expect(() =>
      vpn.getRoutesForVpn([], { routes: ["0.0.0.0/0"] } as any, undefined, undefined, "android"),
    ).toThrow(/不支持全局代理/);
  });

  it("rejects ::/0 and its non-canonical spellings on Android", async () => {
    const vpn = await loadVpnModule();

    for (const route of ["::/0", "0:0:0:0:0:0:0:0/0"]) {
      expect(() =>
        vpn.getRoutesForVpn([], { routes: [route] } as any, undefined, undefined, "android"),
      ).toThrow(/不支持全局代理/);
    }
  });

  it("rejects a default route advertised by a peer proxy_cidr on Android", async () => {
    const vpn = await loadVpnModule();

    expect(() =>
      vpn.getRoutesForVpn(
        [{ peer_id: 1, ipv4_addr: "10.254.1.2", proxy_cidrs: ["0.0.0.0/0"] }] as any,
        {} as any,
        undefined,
        undefined,
        "android",
      ),
    ).toThrow(/不支持全局代理/);
  });

  it("keeps 0.0.0.0/0 legal outside Android (desktop routes all traffic into the mesh)", async () => {
    const vpn = await loadVpnModule();

    const routes = vpn.getRoutesForVpn(
      [],
      { routes: ["0.0.0.0/0"] } as any,
      undefined,
      undefined,
      "windows",
    );

    expect(routes).toEqual(["0.0.0.0/0"]);
  });

  it("does not hand 0.0.0.0/0 to start_vpn and surfaces the reason to the user", async () => {
    useAndroid();
    setConfig("A", false, ["0.0.0.0/0"]);
    setReady("A", "10.0.0.1");
    const vpn = await loadVpnModule();

    await vpn.onNetworkInstanceChange("A");

    expect(mocks.startVpn).not.toHaveBeenCalled();
    expect(vpn.mobileVpnRouteError.value).toMatch(/不支持全局代理/);
    expect(mocks.reportFrontendError).toHaveBeenCalledWith(
      "fe:vpn-routes",
      expect.stringContaining("不支持全局代理"),
    );
  });

  it("starts the VPN once the global route is replaced with a concrete subnet", async () => {
    useAndroid();
    setConfig("A", false, ["0.0.0.0/0"]);
    setReady("A", "10.0.0.1");
    const vpn = await loadVpnModule();

    await vpn.onNetworkInstanceChange("A");
    expect(mocks.startVpn).not.toHaveBeenCalled();

    setConfig("A", false, ["192.168.1.0/24"]);
    await vpn.onNetworkInstanceUpdate("A");

    expect(mocks.startVpn).toHaveBeenCalledTimes(1);
    expect(mocks.startVpn).toHaveBeenCalledWith(
      expect.objectContaining({ routes: expect.arrayContaining(["192.168.1.0/24"]) }),
    );
    expect(vpn.mobileVpnRouteError.value).toBeNull();
  });
});
