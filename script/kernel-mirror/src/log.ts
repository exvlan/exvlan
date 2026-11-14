/** 日志与错误：前缀与旧 shell 脚本完全一致（`[sync] …`），便于既有 grep/看门狗沿用。 */

export class SyncError extends Error {}

export function info(msg: string): void {
  process.stdout.write(`[sync] ${msg}\n`);
}

export function warn(msg: string): void {
  process.stderr.write(`[sync] 警告：${msg}\n`);
}

export function die(msg: string): never {
  process.stderr.write(`[sync] 错误：${msg}\n`);
  throw new SyncError(msg);
}

/** 显式密钥列表（如 MIRROR_TRIGGER_TOKEN）在打印前逐条替换。 */
export function redactSecrets(text: string, secrets: readonly string[] = []): string {
  let out = text;
  for (const secret of secrets) {
    if (secret && secret.length >= 8) {
      out = out.split(secret).join('***');
    }
  }
  return out;
}

/**
 * 通用脱敏：gh token / Bearer / token 字段 / `X-Mirror-Token` 头。
 * 用于 trigger 服务回传 detail（日志尾巴）——绝不把凭据带进 HTTP 响应或日志。
 */
export function redact(text: string): string {
  return text
    .replace(/\b(?:gh[pousr]_[A-Za-z0-9_]{10,}|github_pat_[A-Za-z0-9_]{10,})/g, '***')
    .replace(/(X-Mirror-Token\s*:\s*)\S+/gi, '$1***')
    .replace(/(Authorization\s*:\s*(?:Bearer|Basic|token)\s+)\S+/gi, '$1***')
    .replace(/((?:token|password|secret|gh_token)\s*[=:]\s*?)[^\s"'&,}]+/gi, '$1***');
}
