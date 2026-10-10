import { createExvlanCloudflare } from "../src/index";

const exvlan = createExvlanCloudflare<Env>({
  namespace: (env) => env.EXVLAN_CORE,
  objectName: (request, env) =>
    env.EXVLAN_OBJECT_NAME ?? new URL(request.url).hostname,
  config: (env) => ({
    networkName: env.EXVLAN_NETWORK_NAME,
    networkSecret: env.EXVLAN_NETWORK_SECRET,
    instanceName: "cloudflare-worker",
    encryption: env.EXVLAN_ENABLE_ENCRYPTION !== "false",
  }),
});

export class ExvlanCoreObject extends exvlan.DurableObject {}
export default exvlan;
