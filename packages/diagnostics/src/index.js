// Cloudflare Worker：接收 App 上传的诊断日志，写入私有仓库的 issue。
//
// 设计要点：
// - 目标仓库由 secret `REPO` 固定，**请求体无法指定仓库**（拿到 URL 也改不了投递目标）
// - 需要 `X-Diag-Key` 与 secret `UPLOAD_KEY` 一致，避免被随意灌 issue
// - GitHub issue 正文上限 64K 字符，超限自动拆分多条 issue
//
// secret：GH_TOKEN（fine-grained PAT，Issues: write）、REPO、UPLOAD_KEY

const MAX_BODY = 55000;

function json(data, status = 200) {
  return new Response(JSON.stringify(data), {
    status,
    headers: { 'content-type': 'application/json; charset=utf-8' },
  });
}

function clip(text, limit) {
  if (!text) return '';
  const s = String(text);
  if (s.length <= limit) return s;
  return `${s.slice(0, limit)}\n…（truncated ${s.length - limit} chars）`;
}

function buildSections(payload) {
  const sections = [];
  if (payload.summary) {
    sections.push(['摘要', String(payload.summary)]);
  }
  sections.push(['error.log', clip(payload.errorLog, MAX_BODY)]);
  sections.push(['events.log (tail)', clip(payload.eventsTail, MAX_BODY)]);
  sections.push(['exvlan.log (tail)', clip(payload.kernelTail, MAX_BODY)]);
  return sections.filter(([, body]) => body && body.length > 0);
}

function chunk(sections) {
  const pages = [];
  let current = '';
  for (const [title, body] of sections) {
    const block = `## ${title}\n\n\`\`\`\n${body}\n\`\`\`\n\n`;
    if (current.length + block.length > MAX_BODY && current.length > 0) {
      pages.push(current);
      current = '';
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
  return pages.length > 0 ? pages : ['(empty diagnostics)'];
}

async function createIssue(env, title, body) {
  const resp = await fetch(`https://api.github.com/repos/${env.REPO}/issues`, {
    method: 'POST',
    headers: {
      authorization: `Bearer ${env.GH_TOKEN}`,
      accept: 'application/vnd.github+json',
      'user-agent': 'exvlan-diagnostics-worker',
      'content-type': 'application/json',
    },
    body: JSON.stringify({ title, body, labels: ['diagnostics'] }),
  });
  if (!resp.ok) {
    const detail = await resp.text();
    throw new Error(`github ${resp.status}: ${detail.slice(0, 300)}`);
  }
  const data = await resp.json();
  return data.html_url;
}

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    if (request.method === 'GET' && url.pathname === '/health') {
      return json({ ok: true });
    }
    if (request.method !== 'POST') {
      return json({ error: 'method not allowed' }, 405);
    }
    if (!env.UPLOAD_KEY || request.headers.get('x-diag-key') !== env.UPLOAD_KEY) {
      return json({ error: 'forbidden' }, 403);
    }

    let payload;
    try {
      payload = await request.json();
    } catch {
      return json({ error: 'invalid json' }, 400);
    }

    const version = payload.version || 'unknown';
    const platform = `${payload.os || 'unknown'}-${payload.arch || 'unknown'}`;
    const pages = chunk(buildSections(payload));
    const urls = [];
    try {
      for (let i = 0; i < pages.length; i++) {
        const suffix = pages.length > 1 ? ` (${i + 1}/${pages.length})` : '';
        const title = `[diagnostics] ${version} ${platform}${suffix}`;
        urls.push(await createIssue(env, title, pages[i]));
      }
    } catch (err) {
      return json({ error: String(err) }, 502);
    }
    return json({ urls });
  },
};
