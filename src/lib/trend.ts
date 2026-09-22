import type { HistoryPoint } from "../ipc_contract";

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
