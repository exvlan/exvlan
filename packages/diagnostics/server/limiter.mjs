// 进程内限流：替代 Cloudflare 的 Rate Limiting binding。
//
// 每个实例一个滑动窗口；key 通常是客户端 IP。
// 超过上限返回 { success: false }，与 Worker binding 的返回形状一致。

export function createLimiter(limit, periodSeconds) {
  const windows = new Map(); // key -> number[]（毫秒时间戳）
  const periodMs = periodSeconds * 1000;

  // 定期清理空窗口，避免长期运行时 Map 无限增长。
  const timer = setInterval(
    () => {
      const now = Date.now();
      for (const [key, hits] of windows) {
        const alive = hits.filter((t) => now - t < periodMs);
        if (alive.length === 0) windows.delete(key);
        else windows.set(key, alive);
      }
    },
    Math.max(periodMs, 60_000),
  );
  timer.unref?.();

  return {
    async limit({ key }) {
      const now = Date.now();
      const id = String(key || "unknown");
      const hits = (windows.get(id) || []).filter((t) => now - t < periodMs);
      if (hits.length >= limit) {
        windows.set(id, hits);
        return { success: false };
      }
      hits.push(now);
      windows.set(id, hits);
      return { success: true };
    },
    stop() {
      clearInterval(timer);
    },
  };
}
