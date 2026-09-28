import { pickByteUnit, usageLevel } from "./metrics_math.ts";

const UNITS = ["B", "KB", "MB", "GB", "TB", "PB"];

export const USAGE_LEVEL_COLOR = {
  ok: "#52c41a",
  warn: "#faad14",
  danger: "#ff4d4f",
} as const;

export function formatBytes(bytes: number, digits = 2): string {
  // "没有数据"要说 0 B，不是 "0.00 B"：界面上一排 0.00 会被读成"量不出来"而不是"确实是零"
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  const { value, unit } = pickByteUnit(bytes, UNITS);
  return `${value.toFixed(digits)} ${unit}`;
}

export function formatRate(bytesPerSec: number | null | undefined): string {
  if (bytesPerSec === null || bytesPerSec === undefined) return "—";
  return `${formatBytes(bytesPerSec, 1)}/s`;
}

export function formatPercent(value: number | null | undefined, digits = 1): string {
  if (value === null || value === undefined || !Number.isFinite(value)) return "—";
  return `${value.toFixed(digits)}%`;
}

export function formatGb(bytes: number, digits = 1): string {
  return `${(bytes / 1024 ** 3).toFixed(digits)} GB`;
}

export function formatUptime(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds <= 0) return "—";
  const days = Math.floor(seconds / 86400);
  const hours = Math.floor((seconds % 86400) / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  return days > 0 ? `${days}天 ${hours}小时 ${minutes}分钟` : `${hours}小时 ${minutes}分钟`;
}

/** 状态色：阈值以下绿色，接近阈值黄色，超过红色。 */
export function usageColor(value: number, threshold: number): string {
  const level = usageLevel(value, threshold);
  // 阈值本身不可用（0 / NaN / 被关掉的档位）时不猜颜色，给中性灰
  if (level === null) return "#8c8c8c";
  return USAGE_LEVEL_COLOR[level];
}
