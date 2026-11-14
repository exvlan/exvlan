#!/usr/bin/env bun
/**
 * `/mirror/trigger`：镜像同步的触发与状态服务。
 *
 * 为什么需要它：镜像机每 15 min 拉一轮，发版后客户端最多要等一个周期才看到更新。
 * 发版流水线（`pipeline.yml` → `script/ci/mirror-trigger.sh`）发完 Release 后 POST
 * 一次，镜像机立刻同步；失败不回滚发版（定时同步兜底）。
 *
 * 接口（完整说明见 docs/mirror-toolchain.md）：
 *   POST <path>   Header `X-Mirror-Token: <token>`  → 202 已接受 / 409 正在跑
 *   GET  <path>   Header `X-Mirror-Token: <token>`  → 最近一次状态
 *   GET  /healthz                                   → 容器健康检查（无需鉴权）
 *
 * 鉴权：token 只从 `MIRROR_TRIGGER_TOKEN` 或 `MIRROR_TRIGGER_TOKEN_FILE`（推荐，600）
 * 读取；**绝不**写进仓库/日志/提交，日志与 HTTP 响应一律脱敏。
 *
 * 防重入：内存标志 + 锁文件（O_EXCL，含 pid；超过超时时间视为崩溃遗留并接管）。
 * 单进程同时承担「定时同步」与「按需触发」：两者共用同一把锁，不会并发跑。
 */

import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { redact, redactSecrets, warn } from './src/log';
import { writeFileAtomic } from './src/fsx';

const HERE = import.meta.dir;

export interface TriggerConfig {
  token: string;
  host: string;
  port: number;
  route: string;
  stateFile: string;
  lockFile: string;
  intervalSec: number;
  timeoutSec: number;
  syncEntry: string;
  compatStatusFile: string;
}

interface RunState {
  round: number;
  running: boolean;
  last_run: string | null;
  finished_at: string | null;
  result: 'ok' | 'waiting' | 'error' | null;
  detail: string | null;
  exit_code: number | null;
}

const USAGE = `用法：bun script/kernel-mirror/trigger-server.ts

环境变量：
  MIRROR_TRIGGER_TOKEN         访问令牌（与 MIRROR_TRIGGER_TOKEN_FILE 二选一，后者优先）
  MIRROR_TRIGGER_TOKEN_FILE    令牌文件路径（推荐，权限 600）
  MIRROR_TRIGGER_HOST          监听地址（默认 127.0.0.1，由 Caddy 反代）
  MIRROR_TRIGGER_PORT          监听端口（默认 8899）
  MIRROR_TRIGGER_PATH          HTTP 路径（默认 /mirror/trigger）
  MIRROR_TRIGGER_STATE         状态文件（默认 $TMPDIR/mirror-trigger-state.json）
  MIRROR_TRIGGER_LOCK          锁文件（默认 $TMPDIR/mirror-sync.lock）
  MIRROR_TRIGGER_INTERVAL_SEC  定时同步间隔（默认 900，0 = 只按需触发）
  MIRROR_TRIGGER_TIMEOUT_SEC   单轮同步超时（默认 2700）
  MIRROR_SYNC_ENTRY            同步入口（默认同目录 index.ts）
  MIRROR_STATUS_COMPAT         旧 schema 状态文件（用来回传 waiting/ok）
`;

function envString(name: string, fallback: string): string {
  const value = process.env[name];
  return value === undefined || value === '' ? fallback : value;
}

function envInt(name: string, fallback: number): number {
  const raw = process.env[name];
  if (raw === undefined || raw === '') return fallback;
  const parsed = Number(raw);
  if (!Number.isFinite(parsed) || !Number.isInteger(parsed) || parsed < 0) {
    throw new Error(`${name} 需要非负整数，收到 ${JSON.stringify(raw)}`);
  }
  return parsed;
}

function loadToken(): string {
  const file = envString('MIRROR_TRIGGER_TOKEN_FILE', '');
  let token = '';
  if (file) {
    try {
      token = fs.readFileSync(file, 'utf8').trim();
    } catch (error) {
      throw new Error(`读不到 MIRROR_TRIGGER_TOKEN_FILE=${file}：${error instanceof Error ? error.message : String(error)}`);
    }
  } else {
    token = envString('MIRROR_TRIGGER_TOKEN', '').trim();
  }
  if (!token) {
    throw new Error('缺少 MIRROR_TRIGGER_TOKEN / MIRROR_TRIGGER_TOKEN_FILE（凭据不得写进仓库）');
  }
  if (token.length < 16) {
    throw new Error('MIRROR_TRIGGER_TOKEN 太短（至少 16 字符），请重新生成');
  }
  return token;
}

function resolveConfig(): TriggerConfig {
  const tmp = os.tmpdir();
  return {
    token: loadToken(),
    host: envString('MIRROR_TRIGGER_HOST', '127.0.0.1'),
    port: envInt('MIRROR_TRIGGER_PORT', 8899),
    route: envString('MIRROR_TRIGGER_PATH', '/mirror/trigger'),
    stateFile: envString('MIRROR_TRIGGER_STATE', path.join(tmp, 'mirror-trigger-state.json')),
    lockFile: envString('MIRROR_TRIGGER_LOCK', path.join(tmp, 'mirror-sync.lock')),
    intervalSec: envInt('MIRROR_TRIGGER_INTERVAL_SEC', 900),
    timeoutSec: envInt('MIRROR_TRIGGER_TIMEOUT_SEC', 2700),
    syncEntry: envString('MIRROR_SYNC_ENTRY', path.join(HERE, 'index.ts')),
    compatStatusFile: envString(
      'MIRROR_STATUS_COMPAT',
      path.join(path.dirname(envString('DEST', '/srv/exvlan/core')), 'mirror-status.json'),
    ),
  };
}

function timestamp(): string {
  return new Date().toISOString().replace(/\.\d{3}Z$/, 'Z');
}

/** 恒定时间比较（先摘要再比，长度泄露可忽略）。 */
function tokenMatches(expected: string, provided: string | null): boolean {
  if (!provided) return false;
  const a = Buffer.from(new Bun.CryptoHasher('sha256').update(expected).digest());
  const b = Buffer.from(new Bun.CryptoHasher('sha256').update(provided).digest());
  let diff = 0;
  for (let i = 0; i < a.length; i += 1) diff |= a[i] ^ b[i];
  return diff === 0;
}

export class Runner {
  private readonly cfg: TriggerConfig;
  private readonly state: RunState;
  private child: ReturnType<typeof Bun.spawn> | null = null;
  private outputTail = '';
  private startedAt = '';
  private timedOut = false;

  constructor(cfg: TriggerConfig) {
    this.cfg = cfg;
    this.state = this.loadState();
  }

  private loadState(): RunState {
    const fallback: RunState = {
      round: 0,
      running: false,
      last_run: null,
      finished_at: null,
      result: null,
      detail: null,
      exit_code: null,
    };
    try {
      const raw = JSON.parse(fs.readFileSync(this.cfg.stateFile, 'utf8')) as Partial<RunState>;
      return {
        round: typeof raw.round === 'number' && Number.isFinite(raw.round) ? Math.trunc(raw.round) : 0,
        running: false,
        last_run: typeof raw.last_run === 'string' ? raw.last_run : null,
        finished_at: typeof raw.finished_at === 'string' ? raw.finished_at : null,
        result: raw.result === 'ok' || raw.result === 'waiting' || raw.result === 'error' ? raw.result : null,
        detail: typeof raw.detail === 'string' ? raw.detail : null,
        exit_code: typeof raw.exit_code === 'number' ? Math.trunc(raw.exit_code) : null,
      };
    } catch (error) {
      const code = (error as NodeJS.ErrnoException).code;
      if (code !== 'ENOENT') {
        warn(`读取状态文件失败，按空处理：${error instanceof Error ? error.message : String(error)}`);
      }
      return fallback;
    }
  }

  private persist(): void {
    const snapshot = { ...this.state, running: this.child !== null };
    writeFileAtomic(this.cfg.stateFile, `${JSON.stringify(snapshot, null, 2)}\n`);
  }

  get running(): boolean {
    return this.child !== null;
  }

  snapshot(): Record<string, unknown> {
    return {
      running: this.running,
      last_run: this.state.last_run,
      finished_at: this.state.finished_at,
      result: this.state.result,
      detail: this.state.detail,
      round: this.state.round,
      exit_code: this.state.exit_code,
    };
  }

  private lockHeldByOther(): boolean {
    const stat = (() => {
      try {
        return fs.statSync(this.cfg.lockFile);
      } catch {
        return null;
      }
    })();
    if (!stat) return false;
    if (Date.now() - stat.mtimeMs > this.cfg.timeoutSec * 1000) {
      warn(`发现陈旧的同步锁 ${this.cfg.lockFile}（超过 ${this.cfg.timeoutSec}s），按崩溃遗留处理并接管`);
      fs.rmSync(this.cfg.lockFile, { force: true });
      return false;
    }
    return true;
  }

  private acquireLock(): boolean {
    for (let attempt = 0; attempt < 2; attempt += 1) {
      try {
        const fd = fs.openSync(this.cfg.lockFile, 'wx');
        fs.writeSync(fd, `${process.pid}\n`);
        fs.closeSync(fd);
        return true;
      } catch (error) {
        const code = (error as NodeJS.ErrnoException).code;
        if (code !== 'EEXIST') {
          warn(`创建同步锁失败：${error instanceof Error ? error.message : String(error)}`);
          return false;
        }
        if (!this.lockHeldByOther()) continue;
        return false;
      }
    }
    return false;
  }

  private releaseLock(): void {
    fs.rmSync(this.cfg.lockFile, { force: true });
  }

  /** 触发一轮同步；已在跑返回 false（不排队，避免发版与定时同步叠加）。 */
  start(): boolean {
    if (this.child !== null) return false;
    if (this.lockHeldByOther()) return false;
    if (!this.acquireLock()) return false;

    this.state.round += 1;
    this.state.last_run = timestamp();
    this.state.finished_at = null;
    this.state.result = null;
    this.state.detail = null;
    this.state.exit_code = null;
    this.startedAt = this.state.last_run;
    this.outputTail = '';
    this.timedOut = false;
    this.persist();

    let child: ReturnType<typeof Bun.spawn>;
    try {
      child = Bun.spawn([process.execPath, this.cfg.syncEntry], {
        // 子进程不继承锁环境变量：锁由本进程持有，外部（systemd timer 等）用同一把锁互斥
        env: { ...process.env, MIRROR_SYNC_LOCK: '', MIRROR_TRIGGER_CHILD: '1' },
        stdout: 'pipe',
        stderr: 'pipe',
      });
    } catch (error) {
      this.releaseLock();
      this.state.result = 'error';
      this.state.detail = redact(`启动同步失败：${error instanceof Error ? error.message : String(error)}`);
      this.state.finished_at = timestamp();
      this.persist();
      return false;
    }

    this.child = child;
    void this.collect(child);
    void this.watchdog(child);
    this.armTimeout(child);
    return true;
  }

  /** 超时兜底：先 SIGTERM，10s 后再 SIGKILL。 */
  private armTimeout(child: ReturnType<typeof Bun.spawn>): void {
    const timeoutMs = this.cfg.timeoutSec * 1000;
    if (timeoutMs <= 0) return;
    const timer = setTimeout(() => {
      if (this.child !== child) return;
      this.timedOut = true;
      warn(`同步超过 ${this.cfg.timeoutSec}s，发送 SIGTERM`);
      child.kill('SIGTERM');
      setTimeout(() => {
        if (this.child === child) child.kill('SIGKILL');
      }, 10000);
    }, timeoutMs);
    timer.unref?.();
  }

  private appendOutput(chunk: string): void {
    const text = redactSecrets(redact(chunk), [this.cfg.token]);
    this.outputTail = `${this.outputTail}${text}`.slice(-8000);
  }

  private async collect(child: ReturnType<typeof Bun.spawn>): Promise<void> {
    const read = async (stream: ReadableStream<Uint8Array> | null): Promise<void> => {
      if (!stream) return;
      const decoder = new TextDecoder();
      for await (const chunk of stream) {
        this.appendOutput(decoder.decode(chunk, { stream: true }));
      }
    };
    try {
      await Promise.all([read(child.stdout as ReadableStream<Uint8Array>), read(child.stderr as ReadableStream<Uint8Array>)]);
    } catch (error) {
      this.appendOutput(`\n[trigger] 读取同步输出失败：${error instanceof Error ? error.message : String(error)}\n`);
    }
  }

  private async watchdog(child: ReturnType<typeof Bun.spawn>): Promise<void> {
    const code = await child.exited;
    const timedOut = this.timedOut;
    this.child = null;
    this.state.exit_code = code;
    this.state.finished_at = timestamp();
    this.state.result = code === 0 ? 'ok' : 'error';
    if (timedOut) {
      this.state.result = 'error';
      this.appendOutput(`\n[trigger] 同步超时（>${this.cfg.timeoutSec}s），已终止\n`);
    }
    // 同步脚本自己会把「等待下一轮」写进兼容状态文件；优先采用它的判定
    const fromStatus = this.statusResultAfter(this.startedAt);
    if (fromStatus) this.state.result = fromStatus;
    const tail = this.outputTail.trim();
    const detail = redactSecrets(
      tail === '' ? `退出码 ${code}` : tail.split('\n').slice(-20).join('\n'),
      [this.cfg.token],
    );
    this.state.detail = detail.slice(-4000);
    this.releaseLock();
    this.persist();
    process.stdout.write(`[trigger] round=${this.state.round} 完成：result=${this.state.result} exit=${code}\n`);
  }

  private statusResultAfter(iso: string): 'ok' | 'waiting' | null {
    try {
      const data = JSON.parse(fs.readFileSync(this.cfg.compatStatusFile, 'utf8')) as Record<string, unknown>;
      const checkedAt = typeof data.checked_at === 'string' ? data.checked_at : '';
      if (checkedAt && checkedAt >= iso) {
        if (data.result === 'ok' || data.result === 'waiting') return data.result;
      }
    } catch {
      return null;
    }
    return null;
  }

}

function json(body: Record<string, unknown>, status: number): Response {
  return new Response(`${JSON.stringify(body)}\n`, {
    status,
    headers: { 'Content-Type': 'application/json; charset=utf-8', 'Cache-Control': 'no-store' },
  });
}

export function createServer(cfg: TriggerConfig, runner: Runner): ReturnType<typeof Bun.serve> {
  const route = cfg.route;

  const authorize = (request: Request): boolean =>
    tokenMatches(cfg.token, request.headers.get('X-Mirror-Token'));

  return Bun.serve({
    hostname: cfg.host,
    port: cfg.port,
    async fetch(request: Request): Promise<Response> {
      const url = new URL(request.url);

      if (url.pathname === '/healthz') {
        return json({ ok: true, running: runner.running, round: runner.snapshot().round }, 200);
      }

      if (url.pathname !== route) {
        return json({ ok: false, error: 'not_found' }, 404);
      }

      if (!authorize(request)) {
        // 不区分「缺 token」与「token 错」，也不回显任何片段
        process.stdout.write(`[trigger] 401 ${request.method} ${url.pathname} from ${request.headers.get('x-forwarded-for') ?? 'direct'}\n`);
        return json({ ok: false, error: 'unauthorized' }, 401);
      }

      if (request.method === 'GET') {
        return json({ ok: true, ...runner.snapshot() }, 200);
      }

      if (request.method === 'POST') {
        // 请求体不使用（触发不需要参数）；限制读取大小，避免被灌大包
        const declared = Number(request.headers.get('content-length') ?? '0');
        if (Number.isFinite(declared) && declared > 64 * 1024) {
          return json({ ok: false, error: 'payload_too_large' }, 413);
        }
        if (runner.running) {
          return json({ ok: false, accepted: false, running: true, ...runner.snapshot() }, 409);
        }
        const accepted = runner.start();
        if (accepted) {
          process.stdout.write(`[trigger] round=${runner.snapshot().round} 已启动（POST ${url.pathname}）\n`);
          return json({ ok: true, accepted: true, running: true, ...runner.snapshot() }, 202);
        }
        return json({ ok: false, accepted: false, ...runner.snapshot() }, 409);
      }

      return json({ ok: false, error: 'method_not_allowed' }, 405);
    },
  });
}

function main(argv: readonly string[]): number | null {
  if (argv.includes('-h') || argv.includes('--help')) {
    process.stdout.write(`${USAGE}\n`);
    return 0;
  }

  let cfg: TriggerConfig;
  try {
    cfg = resolveConfig();
  } catch (error) {
    process.stderr.write(`[trigger] 启动失败：${error instanceof Error ? error.message : String(error)}\n`);
    return 2;
  }

  const runner = new Runner(cfg);
  const server = createServer(cfg, runner);
  process.stdout.write(
    `[trigger] 监听 http://${cfg.host}:${server.port}${cfg.route}（定时 ${cfg.intervalSec}s，超时 ${cfg.timeoutSec}s，状态文件 ${cfg.stateFile}）\n`,
  );

  if (cfg.intervalSec > 0) {
    // 启动先跑一轮（与旧容器的行为一致：容器起来就同步，不等第一个周期）
    runner.start();
    const timer = setInterval(() => {
      if (!runner.start()) process.stdout.write('[trigger] 上一轮仍在进行，跳过本轮定时同步\n');
    }, cfg.intervalSec * 1000);
    timer.unref?.();
  }

  const shutdown = (signal: string): void => {
    process.stdout.write(`[trigger] 收到 ${signal}，退出\n`);
    server.stop(true);
    process.exit(0);
  };
  process.on('SIGTERM', () => shutdown('SIGTERM'));
  process.on('SIGINT', () => shutdown('SIGINT'));
  // 服务已接管事件循环：不要调用 process.exit（会把 Bun.serve 立刻终止）
  return null;
}

if (import.meta.main) {
  const code = main(process.argv.slice(2));
  if (code !== null) process.exit(code);
}
