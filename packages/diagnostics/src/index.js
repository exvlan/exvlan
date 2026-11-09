// Cloudflare Worker：接收 App 上传的诊断日志，写入私有仓库的 issue。
//
// 鉴权（7.3 起）：
//   GET  /token   → 签发短期 token（HMAC，绑定请求 IP，10 分钟有效）
//   POST /zip     → Authorization: Bearer <token>
// 客户端不再内置任何长期密钥；服务端密钥为 secret `TOKEN_SECRET`。
//
// 路径：
//   GET  /health       健康检查
//   GET  /token        签发 token（受限流保护）
//   POST /zip          完整 zip（存 KV，3 天过期）+ issue（正文含文本摘要 + 链接）
//   POST /             JSON 文本段（保留接口，同样需要 token）
//   GET  /diag/<id>    下载 zip（随机 id，公开可下，3 天过期）
//
// secret：GH_TOKEN（Issues: write）、REPO、TOKEN_SECRET
// kv：DIAG_KV
// ratelimit bindings：RL_TOKEN（10/分钟/IP）、RL_UPLOAD（20/分钟/IP）

const MAX_BODY = 60000;
const MAX_ZIP = 20 * 1024 * 1024;
const ZIP_TTL_SECONDS = 3 * 24 * 60 * 60;
const TOKEN_TTL_SECONDS = 600;

const enc = new TextEncoder();

function json(data, status = 200) {
  return new Response(JSON.stringify(data), {
    status,
    headers: { "content-type": "application/json; charset=utf-8" },
  });
}

function clip(text, limit) {
  if (!text) return "";
  const s = String(text);
  if (s.length <= limit) return s;
  return `${s.slice(0, limit)}\n…（truncated ${s.length - limit} chars）`;
}

function b64url(bytes) {
  let s = "";
  for (const b of bytes) s += String.fromCharCode(b);
  return btoa(s).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

async function hmac(secret, message) {
  const key = await crypto.subtle.importKey(
    "raw",
    enc.encode(secret),
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign"],
  );
  const sig = await crypto.subtle.sign("HMAC", key, enc.encode(message));
  return b64url(new Uint8Array(sig));
}

async function sha256Hex(text) {
  const digest = await crypto.subtle.digest("SHA-256", enc.encode(text));
  return Array.from(new Uint8Array(digest), (b) => b.toString(16).padStart(2, "0")).join("");
}

function clientIp(request) {
  return request.headers.get("CF-Connecting-IP") || "0.0.0.0";
}

/** token 形如 `v1.<exp>.<nonce>.<sig>`，sig 绑定 exp+nonce+ip 摘要。 */
async function issueToken(env, request) {
  const exp = Math.floor(Date.now() / 1000) + TOKEN_TTL_SECONDS;
  const nonce = randomId().slice(0, 16);
  const ipDigest = await sha256Hex(clientIp(request));
  const sig = await hmac(env.TOKEN_SECRET, `${exp}.${nonce}.${ipDigest}`);
  return { token: `v1.${exp}.${nonce}.${sig}`, exp };
}

async function verifyToken(env, request) {
  const auth = request.headers.get("authorization") || "";
  const token = auth.startsWith("Bearer ") ? auth.slice(7).trim() : "";
  const parts = token.split(".");
  if (parts.length !== 4 || parts[0] !== "v1") {
    return { ok: false, reason: "bad-signature" };
  }
  const [, expRaw, nonce, sig] = parts;
  const exp = Number(expRaw);
  if (!Number.isFinite(exp)) {
    return { ok: false, reason: "bad-signature" };
  }
  if (Math.floor(Date.now() / 1000) > exp) {
    return { ok: false, reason: "expired" };
  }
  const ipDigest = await sha256Hex(clientIp(request));
  const expect = await hmac(env.TOKEN_SECRET, `${exp}.${nonce}.${ipDigest}`);
  if (expect !== sig) {
    return { ok: false, reason: "ip-mismatch" };
  }
  return { ok: true };
}

/**
 * 限流：Rate Limiting binding + KV 计数双保险。
 *
 * 实测该账号的 binding 恒返回 `success: true`（不计数），所以 KV 计数才是
 * 真正生效的那一层；binding 保留，账号/区域恢复正常后自动叠加。
 */
async function rateLimited(env, request, scope, limit) {
  const ip = clientIp(request);
  const binding = scope === "token" ? env.RL_TOKEN : env.RL_UPLOAD;
  if (binding) {
    try {
      const { success } = await binding.limit({ key: ip });
      if (!success) return true;
    } catch {
      // 忽略：下面的 KV 计数兜底
    }
  }
  if (!env.DIAG_KV) return false;
  const bucket = Math.floor(Date.now() / 60000);
  const key = `rl:${scope}:${ip}:${bucket}`;
  try {
    const current = Number((await env.DIAG_KV.get(key)) || 0);
    if (current >= limit) return true;
    await env.DIAG_KV.put(key, String(current + 1), { expirationTtl: 120 });
  } catch {
    // 限流器异常时放行，避免因为基础设施问题全量阻断诊断上传
    return false;
  }
  return false;
}

function buildSections(payload) {
  const sections = [];
  if (payload.summary) {
    sections.push(["摘要", String(payload.summary)]);
  }
  sections.push(["网络自检", clip(payload.networkReport, MAX_BODY)]);
  sections.push(["error.log", clip(payload.errorLog, MAX_BODY)]);
  sections.push(["events.log (tail)", clip(payload.eventsTail, MAX_BODY)]);
  sections.push(["exvlan.log (tail)", clip(payload.kernelTail, MAX_BODY)]);
  return sections.filter(([, body]) => body && body.length > 0);
}

function chunk(sections) {
  const pages = [];
  let current = "";
  for (const [title, body] of sections) {
    const block = `## ${title}\n\n\`\`\`\n${body}\n\`\`\`\n\n`;
    if (current.length + block.length > MAX_BODY && current.length > 0) {
      pages.push(current);
      current = "";
    }
    if (block.length > MAX_BODY) {
      for (let i = 0; i < block.length; i += MAX_BODY) {
        pages.push(block.slice(i, i + MAX_BODY));
      }
    } else {
      current += block;
    }
  }
  if (current.length > 0) pages.push(current);
  return pages.length > 0 ? pages : ["(empty diagnostics)"];
}

async function createIssue(env, title, body, labels) {
  const resp = await fetch(`https://api.github.com/repos/${env.REPO}/issues`, {
    method: "POST",
    headers: {
      authorization: `Bearer ${env.GH_TOKEN}`,
      accept: "application/vnd.github+json",
      "user-agent": "exvlan-diagnostics-worker",
      "content-type": "application/json",
    },
    body: JSON.stringify({ title, body, labels }),
  });
  if (!resp.ok) {
    const detail = await resp.text();
    throw new Error(`github ${resp.status}: ${detail.slice(0, 300)}`);
  }
  const data = await resp.json();
  return data.html_url;
}

function randomId() {
  const bytes = new Uint8Array(16);
  crypto.getRandomValues(bytes);
  return Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
}

function issueTitle(version, platform) {
  return `[diag] ${version} ${platform}`;
}

async function handleToken(request, env) {
  if (!env.TOKEN_SECRET) {
    return json({ error: "token secret not configured" }, 500);
  }
  if (await rateLimited(env, request, "token", 10)) {
    return new Response(JSON.stringify({ error: "rate limited" }), {
      status: 429,
      headers: { "content-type": "application/json", "retry-after": "60" },
    });
  }
  return json(await issueToken(env, request));
}

async function handleZip(request, env, url) {
  const declared = Number(request.headers.get("content-length") || 0);
  if (declared > MAX_ZIP) {
    return json({ error: "zip too large" }, 413);
  }
  const zip = await request.arrayBuffer();
  if (zip.byteLength === 0) {
    return json({ error: "empty zip" }, 400);
  }
  if (zip.byteLength > MAX_ZIP) {
    return json({ error: "zip too large" }, 413);
  }
  if (!env.DIAG_KV) {
    return json({ error: "kv not configured" }, 500);
  }

  const id = randomId();
  await env.DIAG_KV.put(`zip:${id}`, zip, { expirationTtl: ZIP_TTL_SECONDS });

  const version = url.searchParams.get("version") || "unknown";
  const platform = `${url.searchParams.get("os") || "unknown"}-${url.searchParams.get("arch") || "unknown"}`;
  const summary = url.searchParams.get("summary") || "";
  const reason = url.searchParams.get("reason") === "install-failed" ? "install-failed" : "manual";
  const zipUrl = `${url.origin}/diag/${id}`;

  const body = [
    summary ? `## 摘要\n\n\`\`\`\n${clip(summary, 4000)}\n\`\`\`\n` : "",
    `## 完整诊断包\n\n${zipUrl}\n\n> zip 含 error.log、events.log、studio/exvlan/helper 日志、网络自检、ui-state（均已脱敏）。`,
    `> 链接 ${ZIP_TTL_SECONDS / 86400} 天后过期（${zip.byteLength} bytes）。`,
  ]
    .filter(Boolean)
    .join("\n");

  let issueUrl;
  try {
    issueUrl = await createIssue(env, issueTitle(version, platform), body, ["diagnostics", reason]);
  } catch (err) {
    return json({ error: String(err) }, 502);
  }
  return json({ urls: [issueUrl], zipUrl });
}

async function handleDownload(env, id) {
  if (!env.DIAG_KV) {
    return json({ error: "kv not configured" }, 500);
  }
  const zip = await env.DIAG_KV.get(`zip:${id}`, { type: "arrayBuffer" });
  if (!zip) {
    return json({ error: "not found or expired" }, 404);
  }
  return new Response(zip, {
    headers: {
      "content-type": "application/zip",
      "content-disposition": `attachment; filename="exvlan-diagnostics-${id}.zip"`,
      "cache-control": "private, max-age=0, no-store",
    },
  });
}

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    if (request.method === "GET" && url.pathname === "/health") {
      return json({ ok: true });
    }
    if (request.method === "GET" && url.pathname === "/token") {
      return handleToken(request, env);
    }
    if (request.method === "GET" && url.pathname.startsWith("/diag/")) {
      const id = url.pathname.slice("/diag/".length);
      if (!/^[0-9a-f]{32}$/.test(id)) {
        return json({ error: "not found" }, 404);
      }
      return handleDownload(env, id);
    }
    if (request.method !== "POST") {
      return json({ error: "method not allowed" }, 405);
    }

    if (!env.TOKEN_SECRET) {
      return json({ error: "token secret not configured" }, 500);
    }
    const auth = await verifyToken(env, request);
    if (!auth.ok) {
      return json({ error: "unauthorized", reason: auth.reason }, 401);
    }
    if (await rateLimited(env, request, "upload", 20)) {
      return new Response(JSON.stringify({ error: "rate limited" }), {
        status: 429,
        headers: { "content-type": "application/json", "retry-after": "60" },
      });
    }

    if (url.pathname === "/zip") {
      return handleZip(request, env, url);
    }

    let payload;
    try {
      payload = await request.json();
    } catch {
      return json({ error: "invalid json" }, 400);
    }

    const version = payload.version || "unknown";
    const platform = `${payload.os || "unknown"}-${payload.arch || "unknown"}`;
    const pages = chunk(buildSections(payload));
    const urls = [];
    try {
      for (let i = 0; i < pages.length; i++) {
        const suffix = pages.length > 1 ? ` (${i + 1}/${pages.length})` : "";
        urls.push(
          await createIssue(env, `${issueTitle(version, platform)}${suffix}`, pages[i], [
            "diagnostics",
            "manual",
          ]),
        );
      }
    } catch (err) {
      return json({ error: String(err) }, 502);
    }
    return json({ urls });
  },
};
