import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  Commands,
  MonitorEvent,
  type ProcessPage,
  type ProcessQuery,
} from "../ipc_contract";

const EMPTY: ProcessPage = { total: 0, items: [], timestampMs: 0, warming: true };

/** 关键字是逐字符输入的，防抖后再下发给采集循环 */
const QUERY_DEBOUNCE_MS = 250;

/**
 * 进程表只在 `active` 时订阅：后端枚举一次约几十毫秒，不该在别的 Tab 时空转。
 * 过滤/排序/分页由 `query` 下发给后端，前端只持有当前页。
 */
export function useProcessStream(active: boolean, query: ProcessQuery) {
  const [page, setPage] = useState<ProcessPage>(EMPTY);
  const [streaming, setStreaming] = useState(false);

  useEffect(() => {
    if (!active) {
      invoke(Commands.stopProcessStream).catch(() => undefined);
      setStreaming(false);
      return;
    }

    let unlisten: (() => void) | undefined;
    // 每次订阅各自持有 disposed：StrictMode 双挂载时，若用共享 ref 会被后一次挂载复位，
    // 前一次订阅因此逃过清理而永久泄漏。
    let disposed = false;

    const apply = (next: ProcessPage) => {
      if (!disposed) setPage(next);
    };

    listen<ProcessPage>(MonitorEvent.processes, ({ payload }) => apply(payload))
      .then((fn) => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch(() => undefined);

    invoke(Commands.startProcessStream)
      .then(() => !disposed && setStreaming(true))
      .catch(() => undefined);

    // 暖机帧到达前先取一次缓存，避免表格长时间空白。
    invoke<ProcessPage>(Commands.getProcesses)
      .then(apply)
      .catch(() => undefined);

    return () => {
      disposed = true;
      unlisten?.();
      invoke(Commands.stopProcessStream).catch(() => undefined);
    };
  }, [active]);

  // 查询变化时防抖下发；后端发现代数变了会立刻补一帧。
  useEffect(() => {
    const timer = window.setTimeout(() => {
      invoke(Commands.setProcessQuery, { query }).catch(() => undefined);
    }, QUERY_DEBOUNCE_MS);
    return () => window.clearTimeout(timer);
  }, [query]);

  const refresh = useCallback(() => {
    invoke<ProcessPage>(Commands.getProcesses)
      .then(setPage)
      .catch(() => undefined);
  }, []);

  return { page, streaming, refresh };
}
