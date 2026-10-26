import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  Commands,
  MonitorEvent,
  describeError,
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
  // 订阅/启流失败时界面不能只剩一张"没有匹配的进程"的空表 —— 那会把"通路断了"说成"机器上没进程"
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!active) {
      invoke(Commands.stopProcessStream).catch((e) =>
        console.warn("停止进程流失败（接下来切回本页会重新订阅）：", e)
      );
      setStreaming(false);
      return;
    }

    let unlisten: (() => void) | undefined;
    // 每次订阅各自持有 disposed：StrictMode 双挂载时，若用共享 ref 会被后一次挂载复位，
    // 前一次订阅因此逃过清理而永久泄漏。
    let disposed = false;
    const fail = (context: string) => (e: unknown) => {
      console.warn(`${context}：`, e);
      if (!disposed) setError(`${context}：${describeError(e)}`);
    };
    setError(null);

    const apply = (next: ProcessPage) => {
      if (!disposed) setPage(next);
    };

    listen<ProcessPage>(MonitorEvent.processes, ({ payload }) => apply(payload))
      .then((fn) => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch(fail("进程事件订阅失败"));

    invoke(Commands.startProcessStream)
      .then(() => !disposed && setStreaming(true))
      .catch(fail("进程采集启动失败"));

    // 暖机帧到达前先取一次缓存，避免表格长时间空白。
    invoke<ProcessPage>(Commands.getProcesses)
      .then(apply)
      .catch(fail("读取进程列表失败"));

    return () => {
      disposed = true;
      unlisten?.();
      invoke(Commands.stopProcessStream).catch((e) => console.warn("停止进程流失败：", e));
    };
  }, [active]);

  // 查询变化时防抖下发；后端发现代数变了会立刻补一帧。
  useEffect(() => {
    const timer = window.setTimeout(() => {
      invoke(Commands.setProcessQuery, { query }).catch((e) => console.warn("进程筛选没下发：", e));
    }, QUERY_DEBOUNCE_MS);
    return () => window.clearTimeout(timer);
  }, [query]);

  const refresh = useCallback(() => {
    invoke<ProcessPage>(Commands.getProcesses)
      .then((next) => {
        setPage(next);
        // 只清掉自己这次的失败；订阅层断了的错误要等重新订阅才该消失
        setError((prev) => (prev?.startsWith("刷新进程列表失败") ? null : prev));
      })
      .catch((e) => {
        console.warn("刷新进程列表失败：", e);
        setError(`刷新进程列表失败：${describeError(e)}`);
      });
  }, []);

  return { page, streaming, refresh, error };
}
