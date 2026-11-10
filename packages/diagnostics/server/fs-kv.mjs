// 文件版 KV：把 Worker 的 `env.DIAG_KV`（Cloudflare KV）换成服务器本地磁盘。
//
// 只实现 index.js 用到的两个方法：
//   get(key, { type })              → string | ArrayBuffer | null
//   put(key, value, { expirationTtl })
//
// 存储布局：<root>/<base64url(key)>.val + .exp（过期 epoch 秒，缺省=不过期）。
// 启动时与每小时清理一次过期项（zip 默认 3 天，idem 1 小时）。

import { mkdir, readdir, readFile, rm, stat, writeFile } from "node:fs/promises";
import { join } from "node:path";

const b64 = (key) => Buffer.from(String(key), "utf8").toString("base64url");

export function createFsKv(root) {
  const ready = mkdir(root, { recursive: true });

  async function readExp(path) {
    try {
      const text = await readFile(path, "utf8");
      const value = Number(text.trim());
      return Number.isFinite(value) && value > 0 ? value : 0;
    } catch {
      return 0;
    }
  }

  async function removeKey(key) {
    const base = join(root, b64(key));
    await rm(`${base}.val`, { force: true });
    await rm(`${base}.exp`, { force: true });
  }

  return {
    async get(key, options = {}) {
      await ready;
      const base = join(root, b64(key));
      const exp = await readExp(`${base}.exp`);
      if (exp > 0 && exp * 1000 <= Date.now()) {
        await removeKey(key);
        return null;
      }
      let buf;
      try {
        buf = await readFile(`${base}.val`);
      } catch {
        return null;
      }
      if (options.type === "arrayBuffer") {
        return buf.buffer.slice(buf.byteOffset, buf.byteOffset + buf.byteLength);
      }
      return buf.toString("utf8");
    },

    async put(key, value, options = {}) {
      await ready;
      const base = join(root, b64(key));
      const bytes =
        typeof value === "string"
          ? Buffer.from(value, "utf8")
          : Buffer.from(value instanceof ArrayBuffer ? new Uint8Array(value) : value);
      // 先写 value 再写 exp：中途崩溃最多留下「无过期」的孤儿，由 sweep 兜底。
      await writeFile(`${base}.val`, bytes);
      const ttl = Number(options.expirationTtl || 0);
      if (ttl > 0) {
        const exp = Math.floor(Date.now() / 1000) + ttl;
        await writeFile(`${base}.exp`, String(exp));
      } else {
        await rm(`${base}.exp`, { force: true });
      }
    },

    /** 删除过期项；返回 { scanned, removed }。 */
    async sweep() {
      await ready;
      let scanned = 0;
      let removed = 0;
      const entries = await readdir(root).catch(() => []);
      const now = Date.now() / 1000;
      for (const name of entries) {
        if (!name.endsWith(".exp")) continue;
        scanned += 1;
        const base = join(root, name.slice(0, -4));
        const exp = await readExp(base + ".exp");
        if (exp > 0 && exp <= now) {
          await rm(`${base}.val`, { force: true });
          await rm(`${base}.exp`, { force: true });
          removed += 1;
        }
      }
      // 清理「有 val 无 exp」且 mtime 超过 30 天的孤儿（崩溃残留）
      for (const name of entries) {
        if (!name.endsWith(".val")) continue;
        const base = join(root, name.slice(0, -4));
        try {
          await stat(`${base}.exp`);
        } catch {
          const info = await stat(base + ".val").catch(() => null);
          if (info && Date.now() / 1000 - info.mtimeMs / 1000 > 30 * 86400) {
            await rm(base + ".val", { force: true });
            removed += 1;
          }
        }
      }
      return { scanned, removed };
    },
  };
}
