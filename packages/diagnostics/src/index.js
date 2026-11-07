// Cloudflare Worker：接收 App 上传的诊断日志，写入私有仓库的 issue。
//
// 设计要点：
// - 目标仓库由 secret `REPO` 固定，**请求体无法指定仓库**（拿到 URL 也改不了投递目标）
// - 需要 `X-Diag-Key` 与 secret `UPLOAD_KEY` 一致，避免被随意灌 issue
// - 两条上传路径：
//   POST /           JSON 文本段（旧接口，超 64K 自动拆 issue）
//   POST /zip        完整 zip（存 KV，3 天过期，issue 里只贴摘要 + 下载链接）
//   GET  /diag/<id>  下载 zip（随机 id，公开可下）
//
// secret：GH_TOKEN（fine-grained PAT，Issues: write）、REPO、UPLOAD_KEY
// kv：DIAG_KV

const MAX_BODY = 55000;
const MAX_ZIP = 25 * 1024 * 1024; // KV 单值上限
const ZIP_TTL_SECONDS = 3 * 24 * 60 * 60;

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
      // 单段就超限：硬切
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

async function createIssue(env, title, body) {
  const resp = await fetch(`https://api.github.com/repos/${env.REPO}/issues`, {
    method: "POST",
    headers: {
      authorization: `Bearer ${env.GH_TOKEN}`,
      accept: "application/vnd.github+json",
      "user-agent": "exvlan-diagnostics-worker",
      "content-type": "application/json",
    },
    body: JSON.stringify({ title, body, labels: ["diagnostics"] }),
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
  const zipUrl = `${url.origin}/diag/${id}`;

  const body = [
    summary ? `## 摘要\n\n\`\`\`\n${clip(summary, 4000)}\n\`\`\`\n` : "",
    `## 完整诊断包\n\n${zipUrl}\n\n> zip 含 error.log、events.log、内核日志、网络自检、ui-state（均已脱敏）。`,
    `> 链接 ${ZIP_TTL_SECONDS / 86400} 天后过期（${zip.byteLength} bytes）。`,
  ]
    .filter(Boolean)
    .join("\n");

  let issueUrl;
  try {
    issueUrl = await createIssue(env, `[diagnostics] ${version} ${platform}`, body);
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
    if (!env.UPLOAD_KEY || request.headers.get("x-diag-key") !== env.UPLOAD_KEY) {
      return json({ error: "forbidden" }, 403);
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
        const title = `[diagnostics] ${version} ${platform}${suffix}`;
        urls.push(await createIssue(env, title, pages[i]));
      }
    } catch (err) {
      return json({ error: String(err) }, 502);
    }
    return json({ urls });
  },
};
