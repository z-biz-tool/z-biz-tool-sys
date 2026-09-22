import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  Commands,
  MonitorEvent,
  describeError,
  type HistoryPage,
  type HistoryPoint,
  type MetricsSnapshot,
  type StaticInfo,
} from "../ipc_contract";

export type LinkStatus = "connecting" | "live" | "stalled";

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

export function useSystemMonitor(historyLimit = 120, intervalMs = 1000) {
  const [staticInfo, setStaticInfo] = useState<StaticInfo | null>(null);
  const [snapshot, setSnapshot] = useState<MetricsSnapshot | null>(null);
  const [history, setHistory] = useState<HistoryPoint[]>([]);
  const [seedInfo, setSeedInfo] = useState<HistorySeedInfo | null>(null);
  const [status, setStatus] = useState<LinkStatus>("connecting");
  const lastFrameAt = useRef(0);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;

    const push = (frame: MetricsSnapshot) => {
      lastFrameAt.current = Date.now();
      setSnapshot(frame);
      setStatus("live");
      setHistory((prev) => {
        const totalRx = frame.networks.reduce((acc, n) => acc + n.rxBytesPerSec, 0);
        const totalTx = frame.networks.reduce((acc, n) => acc + n.txBytesPerSec, 0);
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
      if (!lastFrameAt.current) return;
      const staleFor = Date.now() - lastFrameAt.current;
      setStatus(staleFor > intervalMs * STALL_FACTOR ? "stalled" : "live");
    }, intervalMs);

    return () => {
      disposed = true;
      unlisten?.();
      window.clearInterval(watchdog);
    };
  }, [historyLimit, intervalMs]);

  return { staticInfo, snapshot, history, status, seedInfo };
}
