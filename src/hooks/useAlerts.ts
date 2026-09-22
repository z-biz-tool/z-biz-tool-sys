import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  Commands,
  MonitorEvent,
  describeError,
  type AlertConfig,
  type AlertEvent,
  type AlertHistoryPage,
} from "../ipc_contract";
import {
  alertIdentity,
  DEFAULT_ALERT_HISTORY_SPAN,
  mergeAlertEvents,
  sameAlertConfig,
} from "../lib/alert";

/** 只在内存里留最近这些条；更早的那部分由后端落盘，靠 `get_alert_history` 回填（T5-03） */
const RECENT_LIMIT = 30;

/**
 * 落盘历史的自述。`storedEvents`（窗口内条数）与 `totalEvents`（文件现存条数）分开显示，
 * 否则"这个窗口里没有"会被读成"从来没告警过"。
 */
export interface AlertHistoryInfo {
  spanSeconds: number;
  /** 窗口内真实条数（后端截断列表之前的数），所以会 ≥ `returnedEvents` */
  storedEvents: number;
  /** 这次实际拿回来的条数 */
  returnedEvents: number;
  totalEvents: number;
  oldestMs: number | null;
  newestMs: number | null;
  unreadableLines: number;
}

export interface AlertsState {
  /** 会话内 + 落盘回填合并后的完整列表，按时间倒序 */
  events: AlertEvent[];
  /** 只来自落盘文件的那些（列表里标"回填"）：它的挂载点是脱敏过的 */
  persisted: AlertEvent[];
  /** 本次会话内的条数：角标 tooltip 要说"本会话"，就不能拿合并后的数糊过去 */
  recentCount: number;
  historyInfo: AlertHistoryInfo | null;
  /** 读历史失败原文（含"存储不可用"）；与实时告警是否工作分开报 */
  historyError: string | null;
  historySpan: number;
  setHistorySpan: (seconds: number) => void;
  refreshHistory: () => void;
  /** 阈值是否已成功下发到后端；false 时界面必须说清楚"当前判定还在用后端已有配置" */
  pushed: boolean;
  /** 阈值下发失败的原文（纯浏览器里没有 IPC 时也会走到这里） */
  syncError: string | null;
  /** `sys://alert` 订阅失败的原文；与下发失败分开报，否则一句错话会把两条链路混成一个结论 */
  listenError: string | null;
}

/**
 * 告警链路的前端半边（T5-01 + T5-03）。
 *
 * 判定全在后端采集循环里，这里做四件事：推阈值、订阅 `sys://alert`、把事件转成 Toast、
 * 以及向 `get_alert_history` 要那份跨重启的落盘记录。之所以不在前端算"连续 N 帧"：
 * Tab 切换、前端节流、页面重载都会把计数打断，那样报出来的告警与后端事实不一致。
 */
export function useAlerts(
  config: AlertConfig,
  onNotice: (event: AlertEvent) => void,
  onCorrected: (effective: AlertConfig) => void
): AlertsState {
  const [recent, setRecent] = useState<AlertEvent[]>([]);
  const [pushed, setPushed] = useState(false);
  const [syncError, setSyncError] = useState<string | null>(null);
  const [listenError, setListenError] = useState<string | null>(null);
  const [stored, setStored] = useState<AlertEvent[]>([]);
  const [historyInfo, setHistoryInfo] = useState<AlertHistoryInfo | null>(null);
  const [historyError, setHistoryError] = useState<string | null>(null);
  const [historySpan, setHistorySpan] = useState(DEFAULT_ALERT_HISTORY_SPAN);
  const [reloadKey, setReloadKey] = useState(0);
  // 用 ref 承接回调：订阅只建立一次，回调换人不该把监听拆了重建（那会丢事件）
  const handlers = useRef({ onNotice, onCorrected });
  useEffect(() => {
    handlers.current = { onNotice, onCorrected };
  }, [onNotice, onCorrected]);

  useEffect(() => {
    let stale = false;
    setPushed(false);
    invoke<AlertConfig>(Commands.setAlertConfig, { config })
      .then((effective) => {
        if (stale) return;
        setSyncError(null);
        setPushed(true);
        // 后端夹取后与界面上显示的不一致时，以生效值覆盖输入框
        if (!sameAlertConfig(effective, config)) handlers.current.onCorrected(effective);
      })
      .catch((e) => {
        if (!stale) setSyncError(describeError(e));
      });
    return () => {
      stale = true;
    };
  }, [config]);

  useEffect(() => {
    let disposed = false;
    let unlisten: UnlistenFn | undefined;
    listen<AlertEvent>(MonitorEvent.alert, ({ payload }) => {
      if (disposed) return;
      setRecent((prev) => [payload, ...prev].slice(0, RECENT_LIMIT));
      handlers.current.onNotice(payload);
      // 后端是同帧先落盘再推送，所以这条已经在文件里了：重读一次让列表与文件对齐
      setReloadKey((k) => k + 1);
    })
      .then((fn) => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch((e) => {
        if (!disposed) setListenError(describeError(e));
      });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    let stale = false;
    invoke<AlertHistoryPage>(Commands.getAlertHistory, { spanSeconds: historySpan })
      .then((page) => {
        if (stale) return;
        setHistoryError(null);
        setStored(page.events);
        setHistoryInfo({
          spanSeconds: page.spanSeconds,
          storedEvents: page.storedEvents,
          returnedEvents: page.events.length,
          totalEvents: page.totalEvents,
          oldestMs: page.oldestMs,
          newestMs: page.newestMs,
          unreadableLines: page.unreadableLines,
        });
      })
      .catch((e) => {
        if (stale) return;
        // 存储不可用时后端明确报错，此时列表退回"只有本次会话"，不静默当成没有历史
        setStored([]);
        setHistoryInfo(null);
        setHistoryError(describeError(e));
      });
    return () => {
      stale = true;
    };
  }, [historySpan, reloadKey]);

  const refreshHistory = useCallback(() => setReloadKey((k) => k + 1), []);

  const events = useMemo(() => mergeAlertEvents(recent, stored), [recent, stored]);
  // 列表里要能分辨"这条是本次会话推的"和"这条是从文件回填的"，两者详略不同（后者挂载点是脱敏的）
  const persisted = useMemo(
    () => stored.filter((e) => !recent.some((r) => alertIdentity(r) === alertIdentity(e))),
    [recent, stored]
  );

  return {
    events,
    persisted,
    recentCount: recent.length,
    historyInfo,
    historyError,
    historySpan,
    setHistorySpan,
    refreshHistory,
    pushed,
    syncError,
    listenError,
  };
}
