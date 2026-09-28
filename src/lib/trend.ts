import type { AlertEvent, AlertLevel, AlertMetric, HistoryPoint } from "../ipc_contract";

/** 趋势图时间窗口选项（T3-06） */
export const TREND_RANGES = [
  { label: "60 秒", value: 60 },
  { label: "5 分钟", value: 300 },
  { label: "1 小时", value: 3600 },
];

/** 1 小时窗口在 0.5 秒采集下最多约 7200 帧，取 2 倍冗余上限 */
export const HISTORY_LIMIT = 14400;

/** Recharts 在上千数据点上会掉帧，先按桶压；桶数上限，输出的行数最多是它的 2 倍（每桶 min+max） */
export const MAX_TREND_POINTS = 240;

/** 曲线上的一个点：`v` 取自真实某一帧，不合成值 */
export interface EnvelopePoint {
  t: number;
  v: number;
}

/**
 * 趋势图的 time 列必须与告警标注用的 time 列逐字相同，否则 ReferenceLine 找不到落点。
 * 这里不用 dayjs：降采样是纯逻辑，要能被 `node --test` 直接跑。
 */
export function formatClock(ms: number): string {
  const d = new Date(ms);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
}

/**
 * 以「最新一帧」为锚点裁剪窗口：数据流停滞时不会把整段历史挤掉，
 * 也不会为了填满曲线而补任何合成点。
 */
export function windowHistory(points: HistoryPoint[], seconds: number): HistoryPoint[] {
  if (!points.length) return points;
  const from = points[points.length - 1].t - seconds * 1000;
  return points.filter((p) => p.t >= from);
}

/** 从一帧历史点里取一条指标的值 */
export type MetricPick = (p: HistoryPoint) => number;

/**
 * 按连续区段取 **min/max 包络**压缩：每桶最多落两个点（最低与最高，各带自己的真实时刻）。
 *
 * 换成包络的理由：14 400 点压到 240 桶时一桶是 60 帧（30 s），
 * 一次 CPU 打满在这 60 帧里只占 1 帧 —— 均值会把它抹成 1.7%，图上看着"从没发生过"。
 * 对监控工具来说那是假结论。包络保住极值，同时点数仍只有桶数的两倍。
 */
export function resampleEnvelope(
  points: HistoryPoint[],
  pick: MetricPick,
  maxBuckets = MAX_TREND_POINTS
): EnvelopePoint[] {
  if (!points.length) return [];
  const size = Math.max(1, Math.ceil(points.length / Math.max(1, maxBuckets)));
  const out: EnvelopePoint[] = [];
  for (let i = 0; i < points.length; i += size) {
    const end = Math.min(i + size, points.length);
    let lo = i;
    let hi = i;
    for (let j = i + 1; j < end; j += 1) {
      if (pick(points[j]) < pick(points[lo])) lo = j;
      if (pick(points[j]) > pick(points[hi])) hi = j;
    }
    if (lo === hi) {
      out.push({ t: points[lo].t, v: pick(points[lo]) });
      continue;
    }
    // 极值要按时间先后落点，否则同一桶里 max 在 min 之前时会画出一段往回走的线
    const a = { t: points[lo].t, v: pick(points[lo]) };
    const b = { t: points[hi].t, v: pick(points[hi]) };
    out.push(a.t <= b.t ? a : b, a.t <= b.t ? b : a);
  }
  return out;
}

/** 降采样结果 → Recharts 的行：time 用秒精度，值保留两位（tooltip 与曲线同一口径） */
export function toChartRows(points: EnvelopePoint[]): Array<{ t: number; time: string; value: number }> {
  return points.map((p) => ({ t: p.t, time: formatClock(p.t), value: Math.round(p.v * 100) / 100 }));
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
