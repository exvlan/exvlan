import type { DurableObject } from "cloudflare:workers";
import type { ExvlanNetworkConfig } from "@exvlan/runtime";

export type CloudflareExvlanConfig = ExvlanNetworkConfig;

export interface ExvlanDurableObjectNamespace {
  getByName(name: string): {
    fetch(request: Request): Promise<Response>;
  };
}

export interface CloudflareExvlanOptions<Env> {
  namespace(env: Env): ExvlanDurableObjectNamespace;
  config(env: Env):
    | CloudflareExvlanConfig
    | Promise<CloudflareExvlanConfig>;
  objectName?: string | ((request: Request, env: Env) => string);
}

export type ExvlanDurableObjectClass<Env> = typeof DurableObject<Env>;

export interface ExvlanCloudflareApplication<Env> {
  readonly DurableObject: ExvlanDurableObjectClass<Env>;
  fetch(
    request: Request,
    env: Env,
    context: ExecutionContext,
  ): Promise<Response>;
}
