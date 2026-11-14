/**
 * `/mirror/trigger` 服务：鉴权、防重入、状态回传、脱敏。
 *
 * 服务在进程内起（`Bun.serve` + port 0），被测的「同步」换成 `test/fixtures/fake-sync.ts`，
 * 因此不需要网络、不需要 gh、也不会碰真实镜像目录。
 */

import { afterAll, beforeAll, describe, expect, test } from 'bun:test';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { Runner, createServer, type TriggerConfig } from '../trigger-server';

const TOKEN = 'test-token-0123456789abcdef';

let root: string;
let cfg: TriggerConfig;
let runner: Runner;
let server: ReturnType<typeof createServer>;
let base: string;

function mkConfig(overrides: Partial<TriggerConfig> = {}): TriggerConfig {
  return {
    token: TOKEN,
    host: '127.0.0.1',
    port: 0,
    route: '/mirror/trigger',
    stateFile: path.join(root, 'state.json'),
    lockFile: path.join(root, 'sync.lock'),
    intervalSec: 0,
    timeoutSec: 20,
    syncEntry: path.join(import.meta.dir, 'fixtures', 'fake-sync.ts'),
    compatStatusFile: path.join(root, 'mirror-status.json'),
    ...overrides,
  };
}

function authHeaders(token = TOKEN): Record<string, string> {
  return { 'X-Mirror-Token': token };
}

async function waitIdle(timeoutMs = 8000): Promise<Record<string, unknown>> {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const response = await fetch(`${base}/mirror/trigger`, { headers: authHeaders() });
    const body = (await response.json()) as Record<string, unknown>;
    if (body.running === false) return body;
    if (Date.now() > deadline) throw new Error('等待同步结束超时');
    await Bun.sleep(50);
  }
}

beforeAll(() => {
  root = fs.mkdtempSync(path.join(os.tmpdir(), 'exvlan-trigger-test-'));
  cfg = mkConfig();
  process.env.MIRROR_TRIGGER_TOKEN = TOKEN;
  process.env.MIRROR_STATUS_COMPAT = cfg.compatStatusFile;
  process.env.FAKE_SYNC_DELAY_MS = '1500';
  runner = new Runner(cfg);
  server = createServer(cfg, runner);
  base = `http://127.0.0.1:${server.port}`;
});

afterAll(() => {
  server.stop(true);
  fs.rmSync(root, { recursive: true, force: true });
});

describe('/mirror/trigger', () => {
  test('GET /healthz 无需鉴权', async () => {
    const response = await fetch(`${base}/healthz`);
    expect(response.status).toBe(200);
    const body = (await response.json()) as Record<string, unknown>;
    expect(body.ok).toBe(true);
  });

  test('未知路径 404', async () => {
    const response = await fetch(`${base}/nope`, { headers: authHeaders() });
    expect(response.status).toBe(404);
  });

  test('缺少 token → 401（不回显任何片段）', async () => {
    const response = await fetch(`${base}/mirror/trigger`, { method: 'POST' });
    expect(response.status).toBe(401);
    const text = await response.text();
    expect(text).not.toContain(TOKEN);
    expect(JSON.parse(text)).toEqual({ ok: false, error: 'unauthorized' });
  });

  test('token 错误 → 401', async () => {
    const response = await fetch(`${base}/mirror/trigger`, {
      method: 'POST',
      headers: authHeaders('wrong-token-0123456789'),
    });
    expect(response.status).toBe(401);
  });

  test('POST 触发同步 → 202 已接受', async () => {
    const response = await fetch(`${base}/mirror/trigger`, { method: 'POST', headers: authHeaders() });
    expect(response.status).toBe(202);
    const body = (await response.json()) as Record<string, unknown>;
    expect(body.accepted).toBe(true);
    expect(body.running).toBe(true);
    expect(body.round).toBe(1);
  });

  test('并发重入 → 409（不排队、不叠加）', async () => {
    const response = await fetch(`${base}/mirror/trigger`, { method: 'POST', headers: authHeaders() });
    expect(response.status).toBe(409);
    const body = (await response.json()) as Record<string, unknown>;
    expect(body.accepted).toBe(false);
    expect(body.running).toBe(true);
  });

  test('跑完回传状态：result=ok + round 递增 + detail 脱敏', async () => {
    const body = await waitIdle();
    expect(body.result).toBe('ok');
    expect(body.round).toBe(1);
    expect(typeof body.last_run).toBe('string');
    const detail = String(body.detail ?? '');
    expect(detail).not.toContain(TOKEN);
    expect(detail).toContain('***');
  });

  test('同步结束后释放锁文件', async () => {
    expect(fs.existsSync(cfg.lockFile)).toBe(false);
  });

  test('再次触发 → 202 且 round 递增', async () => {
    const response = await fetch(`${base}/mirror/trigger`, { method: 'POST', headers: authHeaders() });
    expect(response.status).toBe(202);
    const body = (await response.json()) as Record<string, unknown>;
    expect(body.round).toBe(2);
    const final = await waitIdle();
    expect(final.round).toBe(2);
  });

  test('不支持的方法 → 405', async () => {
    const response = await fetch(`${base}/mirror/trigger`, { method: 'PUT', headers: authHeaders() });
    expect(response.status).toBe(405);
  });

  test('兼容状态文件里是 waiting 时如实回传', async () => {
    process.env.FAKE_SYNC_RESULT = 'waiting';
    const response = await fetch(`${base}/mirror/trigger`, { method: 'POST', headers: authHeaders() });
    expect(response.status).toBe(202);
    const body = await waitIdle();
    expect(body.result).toBe('waiting');
    process.env.FAKE_SYNC_RESULT = 'ok';
  });

  test('外部已持锁（systemd timer）时不抢跑 → 409', async () => {
    fs.writeFileSync(cfg.lockFile, '999999\n');
    const response = await fetch(`${base}/mirror/trigger`, { method: 'POST', headers: authHeaders() });
    expect(response.status).toBe(409);
    fs.rmSync(cfg.lockFile, { force: true });
  });

  test('超时终止：result=error 且 detail 说明超时', async () => {
    const timeoutCfg = mkConfig({
      stateFile: path.join(root, 'state-timeout.json'),
      lockFile: path.join(root, 'sync-timeout.lock'),
      // 独立状态文件：避免读到其它用例刚写的 checked_at（同秒会让比较失真）
      compatStatusFile: path.join(root, 'mirror-status-timeout.json'),
      timeoutSec: 1,
    });
    const timeoutRunner = new Runner(timeoutCfg);
    const timeoutServer = createServer(timeoutCfg, timeoutRunner);
    const timeoutBase = `http://127.0.0.1:${timeoutServer.port}`;
    process.env.FAKE_SYNC_DELAY_MS = '5000';
    try {
      const response = await fetch(`${timeoutBase}/mirror/trigger`, { method: 'POST', headers: authHeaders() });
      expect(response.status).toBe(202);
      let body: Record<string, unknown> = {};
      const deadline = Date.now() + 15000;
      for (;;) {
        const poll = await fetch(`${timeoutBase}/mirror/trigger`, { headers: authHeaders() });
        body = (await poll.json()) as Record<string, unknown>;
        if (body.running === false) break;
        if (Date.now() > deadline) throw new Error('等待超时终止失败');
        await Bun.sleep(100);
      }
      expect(body.result).toBe('error');
      expect(String(body.detail ?? '')).toContain('超时');
    } finally {
      process.env.FAKE_SYNC_DELAY_MS = '1500';
      timeoutServer.stop(true);
      fs.rmSync(timeoutCfg.lockFile, { force: true });
    }
  });
});
