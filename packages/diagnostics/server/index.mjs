// 自建诊断上传服务（Bun）。复用 `../src/index.js` 的 Worker handler，
// 只把 Cloudflare 专有的绑定换成本地实现：
//   DIAG_KV  → 文件存储（30 天保留）
//   RL_*     → 进程内滑动窗口
//   GH_TOKEN → 从 gh CLI 登录态读取（/root/.config/gh/hosts.yml）
//
// 环境变量：
//   DIAG_TOKEN_SECRET  HMAC 密钥（必填，缺失则拒绝启动）
//   DIAG_REPO          默认 exvlan/diagnostics
//   DIAG_DATA_DIR      默认 /srv/diag
//   DIAG_PORT          默认 22100（只绑 127.0.0.1，由 Caddy 反代）
//   DIAG_GH_TOKEN_FILE 默认 /root/.config/gh/hosts.yml

import { readFileSync } from "node:fs";
import { mkdir } from "node:fs/promises";
import worker from "../src/index.js";
import { createFsKv } from "./fs-kv.mjs";
import { createLimiter } from "./limiter.mjs";

const DATA_DIR = process.env.DIAG_DATA_DIR || "/srv/diag";
const PORT = Number(process.env.DIAG_PORT || 22100);
const REPO = process.env.DIAG_REPO || "exvlan/diagnostics";
const GH_TOKEN_FILE = process.env.DIAG_GH_TOKEN_FILE || "/root/.config/gh/hosts.yml";
const TOKEN_SECRET = process.env.DIAG_TOKEN_SECRET || "";

function log(...args) {
  console.log(new Date().toISOString(), ...args);
}

function readGhToken() {
  // hosts.yml 形如：
  //   github.com:
  //       users:
  //           littlezo:
  //               oauth_token: gho_xxx
  try {
    const text = readFileSync(GH_TOKEN_FILE, "utf8");
    const m = text.match(/oauth_token:\s*(\S+)/);
    return m ? m[1] : "";
  } catch (err) {
    log("[diag] read gh token failed:", String(err));
    return "";
  }
}

if (!TOKEN_SECRET) {
  console.error("[diag] DIAG_TOKEN_SECRET 未设置，拒绝启动");
  process.exit(1);
}

const GH_TOKEN = readGhToken();
if (!GH_TOKEN) {
  console.error(`[diag] 未从 ${GH_TOKEN_FILE} 读到 oauth_token，拒绝启动`);
  process.exit(1);
}

await mkdir(DATA_DIR, { recursive: true });

const kv = createFsKv(DATA_DIR);
const env = {
  TOKEN_SECRET,
  GH_TOKEN,
  REPO,
  DIAG_KV: kv,
  RL_TOKEN: createLimiter(10, 60),
  RL_UPLOAD: createLimiter(20, 60),
};

// 启动清理一次，之后每小时一次。
void kv.sweep().then((r) => log(`[diag] sweep at boot: scanned=${r.scanned} removed=${r.removed}`));
const sweepTimer = setInterval(() => {
  void kv.sweep().then((r) => log(`[diag] sweep: scanned=${r.scanned} removed=${r.removed}`));
}, 3600_000);
sweepTimer.unref?.();

const server = Bun.serve({
  hostname: "127.0.0.1",
  port: PORT,
  async fetch(request) {
    const started = Date.now();
    try {
      // Caddy 终结 TLS 后以 http 回源，`new URL(request.url).origin` 会变成
      // `http://<host>`；这里按 X-Forwarded-Proto 还原成 https，保证 issue 里的
      // 链接是 https。
      const proto = request.headers.get("x-forwarded-proto");
      let req = request;
      if (proto === "https") {
        const u = new URL(request.url);
        if (u.protocol === "http:") {
          u.protocol = "https:";
          req = new Request(u.toString(), request);
        }
      }
      const response = await worker.fetch(req, env);
      const url = new URL(request.url);
      log(
        `[diag] ${request.method} ${url.pathname} → ${response.status} ${Date.now() - started}ms`,
      );
      return response;
    } catch (err) {
      log(
        `[diag] ${request.method} ${new URL(request.url).pathname} → 500 ${Date.now() - started}ms`,
        String(err),
      );
      return new Response(JSON.stringify({ error: "internal", detail: String(err) }), {
        status: 500,
        headers: { "content-type": "application/json; charset=utf-8" },
      });
    }
  },
});

log(`[diag] listening on http://127.0.0.1:${server.port} repo=${REPO} data=${DATA_DIR}`);
