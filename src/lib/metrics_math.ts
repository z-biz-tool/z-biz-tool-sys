/**
 * 监控数值的纯算术（无 React、无 IPC），`node --experimental-strip-types --test` 直接跑。
 *
 * 单独成模块是因为这几段原先抄在三处组件里（概览卡、迷你条、采集 hook），
 * 而它们正是"看着对、其实是假结论"的地方：接口速率来自累计计数器差分，
 * 计数器复位/回绕时 naive 减法会给出负数或一个 absurd 的天文数字。
 */

/**
 * 两次累计字节计数之间的增量。
 *
 * `next < prev` 只有两种解释：接口重置（改 MAC、下线重连、驱动重载）或计数器回绕。
 * 两种都不能记成负流量，也不能把差值当成 `u64` 借位后的天文数字 —— 那一帧会把
 * 会话累计与趋势图一起带跑。此时按"从零重新计数"处理，只承认 next 本身。
 */
export function counterDelta(prev: number | null | undefined, next: number): number {
  if (!Number.isFinite(next) || next < 0) return 0;
  if (prev === null || prev === undefined || !Number.isFinite(prev) || prev < 0) return 0;
  if (next < prev) return next;
  return next - prev;
}

/** 累计增量 → 每秒速率；时间窗过短（同一帧重复送达）时不给假速率，直接 0 */
export function counterRatePerSec(
  prev: number | null | undefined,
  next: number,
  elapsedSecs: number
): number {
  if (!Number.isFinite(elapsedSecs) || elapsedSecs < 0.05) return 0;
  return counterDelta(prev, next) / elapsedSecs;
}

/**
 * 多接口速率求和。负值/NaN/Infinity 一律按 0 处理：后端某一条接口读数坏了，
 * 不该让"总下行速率"这一格变成 NaN 或把另一条接口的正常值抵消掉。
 */
export function sumRates(values: Array<number | null | undefined>): number {
  let total = 0;
  for (const v of values) {
    if (typeof v === "number" && Number.isFinite(v) && v > 0) total += v;
  }
  return total;
}

/** 单位换算的最小档：1024 进制，非有限值不许变成 "NaN B" 显示在界面上 */
export function pickByteUnit(bytes: number, units: string[]): { value: number; unit: string } {
  if (!Number.isFinite(bytes) || bytes <= 0) return { value: 0, unit: units[0] };
  const i = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  return { value: bytes / 1024 ** i, unit: units[i] };
}

/**
 * 阈值分档色：≥threshold 红、≥threshold*warnRatio 黄、其余绿。
 * `threshold` 不可用（0/负/NaN）时不猜颜色，返回 null 让调用方显示"未知"。
 */
export function usageLevel(
  value: number,
  threshold: number,
  warnRatio = 0.8
): "ok" | "warn" | "danger" | null {
  if (!Number.isFinite(value)) return null;
  if (!Number.isFinite(threshold) || threshold <= 0) return null;
  if (value >= threshold) return "danger";
  if (value >= threshold * warnRatio) return "warn";
  return "ok";
}
