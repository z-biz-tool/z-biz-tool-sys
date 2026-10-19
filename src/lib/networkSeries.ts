/**
 * per-interface 时序（T3-02）。
 *
 * 为什么要单独一套：后端落盘的历史（`HistoryPoint`）里只有**聚合**的
 * `rx_bytes_per_sec` / `tx_bytes_per_sec`，**没有按接口拆开**。
 * 而 T3-02 的验收标准是「per-interface **实时曲线** + 接口详情」——
 * 聚合曲线回答不了「是哪个网卡在跑流量」这个问题。
 *
 * 所以这里在前端维护一份**每个接口各自一条**的滚动时序：
 * 每一帧采集（含接口名、上下行速率）进来，就往对应接口的队列里追加一个点。
 * 纯逻辑、无 React、无 IPC，可以被 `node --test` 直接跑。
 *
 * 三条硬规矩（与 `lib/trend.ts` 同一套纪律）：
 * 1. **不造点**。数据流停了就停在那儿，曲线不补任何合成值。
 * 2. **点必须来自真实帧**，且带自己的时刻 —— 时刻重复时**替换**而不是追加，
 *    否则图上会出现一堆零宽度的重复点。
 * 3. **有界**。点数与接口数都设上限：docker veth 这类一次性网卡名会不断新增，
 *    没有上限的话内存会一路涨。
 */
import type { MetricsSnapshot } from "../ipc_contract";
// 显式带 .ts 后缀：仓内约定（见 lib/format.ts → "./metrics_math.ts"）。
// 原因很实际：node --experimental-strip-types 跑测试时按 ESM 解析，
// 无后缀的相对路径解析不到 —— 类型导入会被剥掉，值导入不会。
import { formatClock } from "./trend.ts";

/** 时序里的一个点：时刻 + 该接口这一帧的上下行速率（字节/秒） */
export interface NetPoint {
  t: number;
  rx: number;
  tx: number;
}

/** 接口名 → 该接口的时序队列 */
export type NetworkSeries = Map<string, NetPoint[]>;

/** 每个接口最多保留多少个点（1 秒一帧时约 10 分钟） */
export const NET_SERIES_POINTS = 600;

/** 同时保留多少个接口（veth / 临时网卡会不断冒出来） */
export const NET_SERIES_INTERFACES = 32;

function lastTime(points: NetPoint[]): number {
  return points.length ? points[points.length - 1].t : 0;
}

/**
 * 把一帧采集并进时序。**返回新的 Map**，不改动入参。
 *
 * 这一帧里没出现的接口：时序**保留但不再增长**（曲线停在它消失的那一刻）——
 * 不能因为一次抓不到就把历史抹掉，那样 USB 网卡一抖一抖，曲线就一段一段消失。
 */
export function appendNetworkFrame(
  prev: NetworkSeries,
  snapshot: MetricsSnapshot | null,
  maxPoints: number = NET_SERIES_POINTS
): NetworkSeries {
  const next = new Map<string, NetPoint[]>();
  for (const [name, points] of prev) next.set(name, points);
  if (!snapshot || !snapshot.networks?.length) return next;

  for (const nic of snapshot.networks) {
    // 只复制**这一帧要写**的队列：没出现在这一帧里的接口不会被改动，
    // 共享引用是安全的，也省掉了每帧复制全部队列的开销。
    const points = (prev.get(nic.interface) ?? []).slice();
    const point: NetPoint = {
      t: snapshot.timestampMs,
      rx: nic.rxBytesPerSec,
      tx: nic.txBytesPerSec,
    };
    const last = points[points.length - 1];
    if (last && last.t === point.t) {
      // 同一时刻的重复帧：替换成新值（保证图上显示的是最新读数）
      points[points.length - 1] = point;
    } else {
      points.push(point);
    }
    if (points.length > maxPoints) points.splice(0, points.length - maxPoints);
    next.set(nic.interface, points);
  }

  if (next.size > NET_SERIES_INTERFACES) {
    // 超限时丢掉**最久没更新**的那些（而不是最久前创建的）——
    // 判据要看数据新旧，不看名字新旧。
    const ordered = [...next.entries()].sort((a, b) => lastTime(b[1]) - lastTime(a[1]));
    for (const [name] of ordered.slice(NET_SERIES_INTERFACES)) next.delete(name);
  }
  return next;
}

/** 取某个接口的时序（没有就是空数组，不要抛）。 */
export function networkPoints(series: NetworkSeries, name: string | null): NetPoint[] {
  if (!name) return [];
  return series.get(name) ?? [];
}

/** 时序 → 图表行。time 与 `TrendChart` 的 XAxis 对得上（秒精度）。 */
export function networkRows(points: NetPoint[]): Array<{ t: number; time: string; rx: number; tx: number }> {
  return points.map((p) => ({
    t: p.t,
    time: formatClock(p.t),
    rx: Math.round(p.rx * 100) / 100,
    tx: Math.round(p.tx * 100) / 100,
  }));
}

export interface NetworkSummary {
  /** 参与统计的点数 */
  count: number;
  /** 最后一个点的上下行速率 */
  lastRx: number;
  lastTx: number;
  /** 窗口内的峰值 */
  peakRx: number;
  peakTx: number;
  /** 窗口内的平均值 */
  avgRx: number;
  avgTx: number;
}

/** 汇总一段时序。没有点时全部为 0，不返回 NaN（NaN 会让界面显示 "NaN"）。 */
export function summarizeNetwork(points: NetPoint[]): NetworkSummary {
  if (!points.length) {
    return { count: 0, lastRx: 0, lastTx: 0, peakRx: 0, peakTx: 0, avgRx: 0, avgTx: 0 };
  }
  let peakRx = 0;
  let peakTx = 0;
  let sumRx = 0;
  let sumTx = 0;
  for (const p of points) {
    if (p.rx > peakRx) peakRx = p.rx;
    if (p.tx > peakTx) peakTx = p.tx;
    sumRx += p.rx;
    sumTx += p.tx;
  }
  const last = points[points.length - 1];
  return {
    count: points.length,
    lastRx: last.rx,
    lastTx: last.tx,
    peakRx,
    peakTx,
    avgRx: sumRx / points.length,
    avgTx: sumTx / points.length,
  };
}
