import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Commands, MonitorEvent, type ProcessPage } from "../ipc_contract";

const EMPTY: ProcessPage = { total: 0, items: [], timestampMs: 0, warming: true };

/**
 * 进程表只在 `active` 时订阅：后端枚举一次约几十毫秒，不该在别的 Tab 时空转。
 */
export function useProcessStream(active: boolean) {
  const [page, setPage] = useState<ProcessPage>(EMPTY);
  const [streaming, setStreaming] = useState(false);
  const disposed = useRef(false);

  useEffect(() => {
    if (!active) {
      invoke(Commands.stopProcessStream).catch(() => undefined);
      setStreaming(false);
      return;
    }

    let unlisten: (() => void) | undefined;
    disposed.current = false;

    const apply = (next: ProcessPage) => {
      if (!disposed.current) setPage(next);
    };

    listen<ProcessPage>(MonitorEvent.processes, ({ payload }) => apply(payload))
      .then((fn) => {
        if (disposed.current) fn();
        else unlisten = fn;
      })
      .catch(() => undefined);

    invoke(Commands.startProcessStream)
      .then(() => !disposed.current && setStreaming(true))
      .catch(() => undefined);

    // 暖机帧到达前先取一次缓存，避免表格长时间空白。
    invoke<ProcessPage>(Commands.getProcesses)
      .then(apply)
      .catch(() => undefined);

    return () => {
      disposed.current = true;
      unlisten?.();
      invoke(Commands.stopProcessStream).catch(() => undefined);
    };
  }, [active]);

  const refresh = useCallback(() => {
    invoke<ProcessPage>(Commands.getProcesses)
      .then(setPage)
      .catch(() => undefined);
  }, []);

  return { page, streaming, refresh };
}
