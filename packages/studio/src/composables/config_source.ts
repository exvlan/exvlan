/** 配置来源：与内核/协议同一份词汇（`user` / `web`）。 */
export type ConfigSource = "user" | "web";

export function normalizeConfigSource(source: unknown): ConfigSource {
  return source === "web" ? "web" : "user";
}
