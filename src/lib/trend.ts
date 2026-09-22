import type { AlertEvent, AlertLevel, AlertMetric, HistoryPoint } from "../ipc_contract";

/** 趋势图时间窗口选项（T3-06） */
export const TREND_RANGES = [
  { label: "60 秒", value: 60 },
  { label: "5 分钟", value: 300 },
  { label: "1 小时", value: 3600 },
];

/** 1 小时窗口在 0.5 秒采集下最多约 7200 帧，取 2 倍冗余上限 */
export const HISTORY_LIMIT = 14400;

/** Recharts 在上千数据点上会掉帧，先按桶取均值压缩 */
export const MAX_TREND_POINTS = 240;

/**
 * 以「最新一帧」为锚点裁剪窗口：数据流停滞时不会把整段历史挤掉，
 * 也不会为了填满曲线而补任何合成点。
 */
export function windowHistory(points: HistoryPoint[], seconds: number): HistoryPoint[] {
  if (!points.length) return points;
  const from = points[points.length - 1].t - seconds * 1000;
  return points.filter((p) => p.t >= from);
}

/** 点数超限时按连续区段取均值压缩，时间戳取该区段最后一帧 */
export function downsampleHistory(points: HistoryPoint[]): HistoryPoint[] {
  if (points.length <= MAX_TREND_POINTS) return points;
  const size = Math.ceil(points.length / MAX_TREND_POINTS);
  const out: HistoryPoint[] = [];
  for (let i = 0; i < points.length; i += size) {
    const slice = points.slice(i, i + size);
    const avg = (pick: (p: HistoryPoint) => number) =>
      slice.reduce((a, p) => a + pick(p), 0) / slice.length;
    out.push({
      t: slice[slice.length - 1].t,
      cpu: avg((p) => p.cpu),
      memory: avg((p) => p.memory),
      rxBytesPerSec: avg((p) => p.rxBytesPerSec),
      txBytesPerSec: avg((p) => p.txBytesPerSec),
    });
  }
  return out;
}

/** 趋势图上的一条告警竖线 */
export interface TrendMarker {
  /** 直接取自曲线里某一行的 time —— 竖线只能落在真实存在的那个点上 */
  x: string;
  /** 被对齐到的采样时刻 */
  snappedTo: number;
  /** 告警自己的时刻，与 snappedTo 的差就是这条线的定位误差 */
  alertAt: number;
  level: AlertLevel;
  /** 同一格聚合了几条同档告警 */
  count: number;
}

export interface AlertMarkerResult {
  markers: TrendMarker[];
  /** 落在曲线覆盖范围之外、因而图上画不出来的条数 */
  outside: number;
  /** 画出的线里最大的定位误差（毫秒）；没有线时为 0 */
  maxSkewMs: number;
}

/**
 * 把告警事件对齐到趋势曲线的采样点上（T5-03）。
 *
 * 两条硬规矩：
 * 1. **不造点**。曲线在某个时刻没有采样，那条告警就不画，只计入 `outside` ——
 *    画一根落在插值位置的线，等于让读者以为那里有数据。
 * 2. 定位误差必须说出来（`maxSkewMs`）。1 小时窗口压缩到 240 点后一格是 15 s，
 *    界面拿它去标"这条线只代表这一格"，而不是假装精确到秒。
 */
export function alignAlertMarkers(
  events: AlertEvent[],
  rows: Array<{ t: number; time: string }>,
  metric: AlertMetric
): AlertMarkerResult {
  const targets = events.filter((e) => e.metric === metric);
  if (!rows.length || !targets.length) {
    return { markers: [], outside: targets.length, maxSkewMs: 0 };
  }
  const from = rows[0].t;
  const to = rows[rows.length - 1].t;
  const byRow = new Map<number, TrendMarker>();
  let outside = 0;
  let maxSkewMs = 0;

  for (const event of targets) {
    if (event.timestampMs < from || event.timestampMs > to) {
      outside += 1;
      continue;
    }
    let best = 0;
    let bestSkew = Math.abs(rows[0].t - event.timestampMs);
    for (let i = 1; i < rows.length; i += 1) {
      const skew = Math.abs(rows[i].t - event.timestampMs);
      if (skew < bestSkew) {
        best = i;
        bestSkew = skew;
      }
    }
    maxSkewMs = Math.max(maxSkewMs, bestSkew);
    const existing = byRow.get(best);
    if (existing && existing.level === event.level) existing.count += 1;
    else if (!existing) {
      byRow.set(best, {
        x: rows[best].time,
        snappedTo: rows[best].t,
        alertAt: event.timestampMs,
        level: event.level,
        count: 1,
      });
    }
  }

  const markers = [...byRow.values()].sort((a, b) => a.snappedTo - b.snappedTo);
  return { markers, outside, maxSkewMs };
}
