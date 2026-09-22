import { useCallback, useEffect, useMemo, useState } from "react";
import { INTERVAL_OPTIONS, PREF } from "../lib/ui";
import { TREND_RANGES } from "../lib/trend";
import { DEFAULT_ALERT_CONFIG, normalizeAlertConfig } from "../lib/alert";
import { buildPrefsSnapshot } from "../lib/prefs_file";
import type { AlertConfig, PrefsSnapshot } from "../ipc_contract";

/**
 * 用户偏好的读写收口（T4-02）。刻意不引状态库：项少、且没有跨层共享需求。
 *
 * 读侧一律过白名单：陌生/旧版本残留值不能让界面塌掉 —— 曾实测一个不认识的 `tab`
 * key 会让 antd 一个面板都不渲染（内容区仅 67 字符），而陌生 `interval` 会算错
 * "停滞"阈值从而永不报停滞。落盘走同一个 `PREF` 前缀，T5-11 的导入/导出正是从这里
 * 取快照（`prefsSnapshot`）和整份写回（`applyPrefs`），值语义都在 `lib/prefs_file.ts`。
 */
export function usePrefs() {
  const [darkMode, setDarkMode] = useState(() => localStorage.getItem(PREF + "dark") === "1");
  const [intervalMs, setIntervalMs] = useState(() => {
    const saved = Number(localStorage.getItem(PREF + "interval"));
    return INTERVAL_OPTIONS.some((o) => o.value === saved) ? saved : 1000;
  });
  const [trendRange, setTrendRange] = useState<number>(() => {
    const saved = Number(localStorage.getItem(PREF + "trendRange"));
    return TREND_RANGES.some((r) => r.value === saved) ? saved : 60;
  });
  // Tab 的陌生值要按真实 `tabItems` 回落，那份列表在渲染期才拿得到，故校验留在 App 内
  const [activeTab, setActiveTab] = useState(() => localStorage.getItem(PREF + "tab") ?? "overview");
  // 告警阈值（T5-01）：一份 JSON，读侧整体过 normalizeAlertConfig，写侧回写归一化后的值
  const [alertConfig, setAlertConfig] = useState<AlertConfig>(() => {
    const saved = localStorage.getItem(PREF + "alert");
    if (!saved) return DEFAULT_ALERT_CONFIG;
    try {
      return normalizeAlertConfig(JSON.parse(saved));
    } catch {
      return DEFAULT_ALERT_CONFIG;
    }
  });

  useEffect(() => {
    localStorage.setItem(PREF + "dark", darkMode ? "1" : "0");
  }, [darkMode]);
  useEffect(() => {
    localStorage.setItem(PREF + "interval", String(intervalMs));
  }, [intervalMs]);
  useEffect(() => {
    localStorage.setItem(PREF + "trendRange", String(trendRange));
  }, [trendRange]);
  useEffect(() => {
    localStorage.setItem(PREF + "alert", JSON.stringify(alertConfig));
  }, [alertConfig]);

  /**
   * 导出用的当前快照（T5-11）。就是这几项，不含任何路径、进程或主机信息。
   * 页签一项由调用方按界面上真正生效的那个 key 覆盖（见 App 里的 `activeKey`）。
   */
  const snapshot = useMemo<PrefsSnapshot>(
    () => buildPrefsSnapshot({ darkMode, intervalMs, trendRangeSecs: trendRange, activeTab, alertConfig }),
    [darkMode, intervalMs, trendRange, activeTab, alertConfig]
  );

  /**
   * 整份替换（导入的第二步）。刻意走回同一批 setter：上面的 useEffect 会把五项一起落盘，
   * 不存在"界面变了、localStorage 还是旧的"的中间态。告警阈值变了还会由 `useAlerts` 再下发给后端。
   */
  const applyPrefs = useCallback((next: PrefsSnapshot) => {
    setDarkMode(next.darkMode);
    setIntervalMs(next.intervalMs);
    setTrendRange(next.trendRangeSecs);
    setActiveTab(next.activeTab);
    setAlertConfig(next.alert);
  }, []);

  return {
    darkMode,
    setDarkMode,
    intervalMs,
    setIntervalMs,
    trendRange,
    setTrendRange,
    activeTab,
    setActiveTab,
    alertConfig,
    setAlertConfig,
    prefsSnapshot: snapshot,
    applyPrefs,
  };
}
