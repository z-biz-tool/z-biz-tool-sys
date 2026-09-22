const UNITS = ["B", "KB", "MB", "GB", "TB", "PB"];

export function formatBytes(bytes: number, digits = 2): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  const i = Math.min(
    Math.floor(Math.log(bytes) / Math.log(1024)),
    UNITS.length - 1
  );
  return `${(bytes / 1024 ** i).toFixed(digits)} ${UNITS[i]}`;
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
  if (value >= threshold) return "#ff4d4f";
  if (value >= threshold * 0.8) return "#faad14";
  return "#52c41a";
}
