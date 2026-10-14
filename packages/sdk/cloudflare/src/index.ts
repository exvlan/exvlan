import coreModule from "./generated/exvlan_core.wasm";

import { createCloudflareApplication } from "./app.js";
import type {
  CloudflareExvlanOptions,
  ExvlanCloudflareApplication,
} from "./types.js";

export type {
  CloudflareExvlanConfig,
  CloudflareExvlanOptions,
  ExvlanCloudflareApplication,
} from "./types.js";

export function createExvlanCloudflare<Env>(
  options: CloudflareExvlanOptions<Env>,
): ExvlanCloudflareApplication<Env> {
  return createCloudflareApplication(coreModule, options);
}
