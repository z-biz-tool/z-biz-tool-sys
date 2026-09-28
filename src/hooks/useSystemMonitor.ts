import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { counterDelta, sumRates } from "../lib/metrics_math";
import {
  Commands,
  MonitorEvent,
  describeError,
  type HistoryPage,
  type HistoryPoint,
  type MetricsSnapshot,
  type StaticInfo,
} from "../ipc_contract";

export type LinkStatus = "connecting" | "live" | "stalled" | "paused";

export interface MetricsResponse {
  snapshot: MetricsSnapshot | null;
}

/** 落盘历史的回填跨度：与趋势图的最大窗口一致，重启后 1 小时曲线立刻有数据 */
const SEED_SPAN_SECONDS = 3600;

const STALL_FACTOR = 3;

/**
 * 已落盘的历史如何参与当前曲线的说明；`points` 为 0 表示这条曲线全是本次会话的实时点。
 */
export interface HistorySeedInfo {
  points: number;
  bucketSeconds: number;
  error: string | null;
}

/** 本次会话以来各网卡累计收发的字节数（接口第一次出现之前的一段不计入） */
export interface NetSessionTotals {
  rxBytes: number;
  txBytes: number;
}

/** 本次会话以来各网卡累计收发的字节数（网卡第一次出现之前的那一段不计） */
export interface NetSessionTotals {
  rxBytes: number;
  txBytes: number;
}

export function useSystemMonitor(historyLimit = 120, intervalMs = 1000, paused = false) {
  const [staticInfo, setStaticInfo] = useState<StaticInfo | null>(null);
  const [snapshot, setSnapshot] = useState<MetricsSnapshot | null>(null);
  const [history, setHistory] = useState<HistoryPoint[]>([]);
  const [seedInfo, setSeedInfo] = useState<HistorySeedInfo | null>(null);
  const [status, setStatus] = useState<LinkStatus>("connecting");
  const [netSession, setNetSession] = useState<NetSessionTotals | null>(null);
  const lastFrameAt = useRef(0);
  // 暂停用 ref 而不是进订阅的依赖：重建订阅会把已建立的历史/监听链拆一次，
  // 而"暂停"只是不采纳帧，不该让曲线重订阅。
  const pausedRef = useRef(paused);
  useEffect(() => {
    pausedRef.current = paused;
    setStatus(paused ? "paused" : "connecting");
  }, [paused]);
  // 每条网卡第一次见到的累计字节数，作为本次会话的基线（接口复位时换基线）
  const countersBase = useRef<Map<string, { rx: number; tx: number }>>(new Map());

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    // 重挂载（StrictMode 双挂载、intervalMs 变化重建订阅）要从零基线重新开始，
    // 否则同一份累计计数会被两条链各算一次
    countersBase.current.clear();

    const push = (frame: MetricsSnapshot) => {
      // 暂停期间在途的那一帧直接丢掉：曲线要停在按下 Space 的那一刻，
      // 否则"已暂停"的标签配一条还在动的线，等于两个都说自己在骗人
      if (pausedRef.current) return;
      lastFrameAt.current = Date.now();
      setSnapshot(frame);
      setStatus("live");
      const totalRx = sumRates(frame.networks.map((n) => n.rxBytesPerSec));
      const totalTx = sumRates(frame.networks.map((n) => n.txBytesPerSec));
      // 会话累计走累计计数器的差分，不是把每秒速率加回去：0.5 s 一帧的瞬时值
      // 会在采集抖动时漏掉整段流量，累计计数器不会。
      let sessionRx = 0;
      let sessionTx = 0;
      for (const n of frame.networks) {
        const seen = countersBase.current.get(n.interface);
        if (!seen) {
          countersBase.current.set(n.interface, { rx: n.totalReceivedBytes, tx: n.totalTransmittedBytes });
          continue;
        }
        // next < prev 是计数器复位/回绕：counterDelta 只承认 next 本身，并就地换基线
        if (n.totalReceivedBytes < seen.rx) seen.rx = n.totalReceivedBytes;
        if (n.totalTransmittedBytes < seen.tx) seen.tx = n.totalTransmittedBytes;
        sessionRx += counterDelta(seen.rx, n.totalReceivedBytes);
        sessionTx += counterDelta(seen.tx, n.totalTransmittedBytes);
      }
      setNetSession({ rxBytes: sessionRx, txBytes: sessionTx });
      setHistory((prev) => {
        const next = prev.concat({
          t: frame.timestampMs,
          cpu: frame.cpu.total,
          memory: frame.memory.usagePercent,
          rxBytesPerSec: totalRx,
          txBytesPerSec: totalTx,
        });
        return next.length > historyLimit ? next.slice(next.length - historyLimit) : next;
      });
    };

    invoke<StaticInfo>(Commands.getStaticInfo)
      .then((info) => {
        if (!disposed) setStaticInfo(info);
      })
      .catch(() => {
        /* 静态信息缺失时界面保持占位，不伪造数据 */
      });

    // 事件订阅建立前可能已有帧，先取一次缓存兜底。
    invoke<MetricsResponse>(Commands.getMetricsSnapshot)
      .then((res) => {
        if (!disposed && res.snapshot) push(res.snapshot);
      })
      .catch(() => undefined);

    // 回填已落盘的历史：否则重启后 1 小时窗口要重新攒满一小时才有数据。
    const mountedAt = Date.now();
    invoke<HistoryPage>(Commands.getHistory, { spanSeconds: SEED_SPAN_SECONDS })
      .then((page) => {
        if (disposed) return;
        // 只回填早于本次会话起点、且早于首个实时点的部分：
        // 同一时刻留两份值会让曲线出现竖直折返，晚于会话起点的点实时链路本来就有。
        const seedPoints = page.points.filter((p) => p.t < mountedAt);
        setSeedInfo({
          points: seedPoints.length,
          bucketSeconds: page.bucketSeconds,
          error: null,
        });
        if (!seedPoints.length) return;
        setHistory((prev) => {
          const oldestLive = prev.length ? prev[0].t : Number.POSITIVE_INFINITY;
          const usable = seedPoints.filter((p) => p.t < oldestLive);
          if (!usable.length) return prev;
          const merged = usable.concat(prev);
          return merged.length > historyLimit
            ? merged.slice(merged.length - historyLimit)
            : merged;
        });
      })
      .catch((e) => {
        // 存储不可用（应用数据目录写不了）时曲线照常，只是没有跨重启的那段
        if (!disposed) {
          setSeedInfo({ points: 0, bucketSeconds: 0, error: describeError(e) });
        }
      });

    listen<MetricsSnapshot>(MonitorEvent.metrics, ({ payload }) => {
      if (!disposed) push(payload);
    })
      .then((fn) => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch(() => {
        if (!disposed) setStatus("stalled");
      });

    const watchdog = window.setInterval(() => {
      if (!lastFrameAt.current || pausedRef.current) return;
      const staleFor = Date.now() - lastFrameAt.current;
      setStatus(staleFor > intervalMs * STALL_FACTOR ? "stalled" : "live");
    }, intervalMs);

    return () => {
      disposed = true;
      unlisten?.();
      window.clearInterval(watchdog);
    };
  }, [historyLimit, intervalMs]);

  return { staticInfo, snapshot, history, status, seedInfo, netSession };
}
