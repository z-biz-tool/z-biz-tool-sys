import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  Commands,
  MonitorEvent,
  type MetricsSnapshot,
  type StaticInfo,
} from "../ipc_contract";

export interface HistoryPoint {
  t: number;
  cpu: number;
  memory: number;
  rxBytesPerSec: number;
  txBytesPerSec: number;
}

export type LinkStatus = "connecting" | "live" | "stalled";

export interface MetricsResponse {
  snapshot: MetricsSnapshot | null;
}

const STALL_FACTOR = 3;

export function useSystemMonitor(historyLimit = 120, intervalMs = 1000) {
  const [staticInfo, setStaticInfo] = useState<StaticInfo | null>(null);
  const [snapshot, setSnapshot] = useState<MetricsSnapshot | null>(null);
  const [history, setHistory] = useState<HistoryPoint[]>([]);
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

  return { staticInfo, snapshot, history, status };
}
