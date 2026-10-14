import { createExvlanCloudflare } from "@exvlan/cloudflare";

const exvlan = createExvlanCloudflare<Env>({
  namespace: (env) => env.EXVLAN_CORE,
  config: (env) => ({
    networkName: env.EXVLAN_NETWORK_NAME,
    networkSecret: env.EXVLAN_NETWORK_SECRET,
    instanceName: "exvlan-api-example-relay",
    encryption: true,
  }),
});

export class ExvlanCoreObject extends exvlan.DurableObject {}
export default exvlan;
