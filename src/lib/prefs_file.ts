/**
 * 偏好导入/导出的**值语义**收口（T5-11）。
 *
 * 后端 `prefs.rs` 只管落盘边界（路径白名单、体积上限、原子写、"这是不是本应用这份格式"），
 * 把 `prefs` 当一个不透明对象透传；这里则复用 `usePrefs` 那套读侧白名单，
 * 所以"什么值算合法"在整个应用里仍然只有一个口径 —— 加一项偏好只要改 `PREFS_KEYS` 和这一段。
 *
 * 导入的文件是**用户可编辑**的，因此任何一项都可能缺失、类型不对、或越界。
 * 规则：认得的值原样采纳；能夹的夹进合法集合；认不出的**保持现状**，
 * 绝不因为一个脏值把界面打回空白（那是本仓库真踩过的坑，见 `usePrefs` 顶部注释）。
 */
import { ALERT_METRIC_ORDER, normalizeAlertConfig } from "./alert";
import { TREND_RANGES } from "./trend";
import { INTERVAL_OPTIONS } from "./ui";
import type { AlertConfig, PrefsSnapshot } from "../ipc_contract";

export type PrefsKey = keyof PrefsSnapshot;

/** 导出/导入的完整键集合；顺序即界面展示顺序 */
export const PREFS_KEYS: PrefsKey[] = [
  "darkMode",
  "intervalMs",
  "trendRangeSecs",
  "activeTab",
  "alert",
];

export const PREFS_LABELS: Record<PrefsKey, string> = {
  darkMode: "深色主题",
  intervalMs: "采集间隔",
  trendRangeSecs: "趋势窗口",
  activeTab: "默认页签",
  alert: "告警阈值",
};

/**
 * 单项的来源判定：
 * - `adopted`：文件里就是合法值，原样采纳
 * - `clamped`：文件里有这一项，但被折回合法区间/合法集合（如间隔 1500 → 1 秒）
 * - `absent`：文件里没有这一项，保持当前值（部分导出的手改文件属于这种）
 * - `invalid`：这一项类型就不对（如 `darkMode: "yes"`），保持当前值
 */
export type PrefsOutcome = "adopted" | "clamped" | "absent" | "invalid";

export interface PrefsEntry {
  key: PrefsKey;
  label: string;
  outcome: PrefsOutcome;
  /** 采纳后的值，人类可读 */
  text: string;
  /** 采纳后的值是否真的与当前值不同 */
  differs: boolean;
  /**
   * 文件里这一项的原值。界面对"已夹取""值不可用"都要能说出它原本是什么 ——
   * 只报"无效"而不报是什么无效，用户没法回头改文件。
   */
  raw: unknown;
}

export interface PrefsInterpretation {
  /** 归一化后的完整偏好，可直接整体落盘 */
  next: PrefsSnapshot;
  entries: PrefsEntry[];
  /** 文件里有、但这个版本不认的键（只报告，绝不写入） */
  unknownKeys: string[];
  adopted: number;
  clamped: number;
  /** 保持现状的项数：缺项 + 脏值 */
  kept: number;
  /** 采纳后与当前值真正不同的项数；0 表示"这份文件就是现在的配置" */
  changed: number;
}

const DEFAULT_INTERVAL_MS = 1000;
const DEFAULT_TREND_SECS = 60;
const DEFAULT_TAB = "overview";

const labelOf = (
  options: { value: number; label: string }[],
  value: number
): string => options.find((option) => option.value === value)?.label ?? `${value}`;

function alertText(config: AlertConfig): string {
  return `${config.enabled ? "开启" : "静默"}｜连续 ${config.consecutive} 帧｜冷却 ${config.cooldownSecs} s｜CPU ${config.cpu.warning}/${config.cpu.critical}｜内存 ${config.memory.warning}/${config.memory.critical}｜磁盘 ${config.disk.warning}/${config.disk.critical}`;
}

/**
 * 告警配置有没有被"动过手脚"。逐叶子比，不用 JSON 串比 ——
 * 手改文件里键的顺序不同不算夹取。
 */
function alertWasFolded(raw: unknown, next: AlertConfig): boolean {
  if (typeof raw !== "object" || raw === null) return true;
  const source = raw as Record<string, unknown>;
  if (typeof source.enabled !== "boolean" || source.enabled !== next.enabled) return true;
  if (typeof source.consecutive !== "number" || source.consecutive !== next.consecutive) return true;
  if (typeof source.cooldownSecs !== "number" || source.cooldownSecs !== next.cooldownSecs) return true;
  return ALERT_METRIC_ORDER.some((metric) => {
    const thresholds = source[metric];
    if (typeof thresholds !== "object" || thresholds === null) return true;
    const t = thresholds as Record<string, unknown>;
    return (
      typeof t.warning !== "number" ||
      t.warning !== next[metric].warning ||
      typeof t.critical !== "number" ||
      t.critical !== next[metric].critical
    );
  });
}

/** 展示用的原值：字符串直接显，其余走 JSON；过长截断。缺项显 `—`。 */
export const rawValueText = (value: unknown): string => {
  if (value === undefined) return "—";
  const text = typeof value === "string" ? value : (JSON.stringify(value) ?? String(value));
  return text.length > 120 ? `${text.slice(0, 120)}…` : text;
};

/** 导出用的快照：`alert` 再归一化一次，保证写出去的文件读回来必定是同一个值。 */
export function buildPrefsSnapshot(input: {
  darkMode: boolean;
  intervalMs: number;
  trendRangeSecs: number;
  activeTab: string;
  alertConfig: unknown;
}): PrefsSnapshot {
  return {
    darkMode: input.darkMode,
    intervalMs: INTERVAL_OPTIONS.some((option) => option.value === input.intervalMs)
      ? input.intervalMs
      : DEFAULT_INTERVAL_MS,
    trendRangeSecs: TREND_RANGES.some((range) => range.value === input.trendRangeSecs)
      ? input.trendRangeSecs
      : DEFAULT_TREND_SECS,
    activeTab: input.activeTab,
    alert: normalizeAlertConfig(input.alertConfig),
  };
}

/**
 * 把一份文件里的 `prefs` 与当前配置合成下一份配置，并逐项说清来源。
 * 只读入参、不产生副作用，因此在界面里预览时可以直接调用。
 */
export function interpretImportedPrefs(
  filePrefs: unknown,
  current: PrefsSnapshot,
  /** 真实页签 key 列表：由 App 的 `tabItems` 传入，这里不另写一份，否则迟早漂移 */
  tabs: string[]
): PrefsInterpretation {
  const source = (
    typeof filePrefs === "object" && filePrefs !== null ? filePrefs : {}
  ) as Record<string, unknown>;

  const next: PrefsSnapshot = { ...current, alert: normalizeAlertConfig(current.alert) };
  const entries: PrefsEntry[] = [];

  const push = (
    key: PrefsKey,
    outcome: PrefsOutcome,
    text: string,
    raw: unknown,
    differs: boolean
  ) => {
    entries.push({ key, label: PREFS_LABELS[key], outcome, text, differs, raw });
  };

  // 深色主题
  {
    const raw = source.darkMode;
    if (raw === undefined) {
      push("darkMode", "absent", current.darkMode ? "开" : "关", raw, false);
    } else if (typeof raw !== "boolean") {
      push("darkMode", "invalid", current.darkMode ? "开" : "关", raw, false);
    } else {
      next.darkMode = raw;
      push("darkMode", "adopted", raw ? "开" : "关", raw, raw !== current.darkMode);
    }
  }

  // 采集间隔：只认 INTERVAL_OPTIONS，与 `usePrefs` 读侧同一判据
  {
    const raw = source.intervalMs;
    if (raw === undefined) {
      push("intervalMs", "absent", labelOf(INTERVAL_OPTIONS, current.intervalMs), raw, false);
    } else if (typeof raw !== "number" || !Number.isFinite(raw)) {
      push("intervalMs", "invalid", labelOf(INTERVAL_OPTIONS, current.intervalMs), raw, false);
    } else if (INTERVAL_OPTIONS.some((option) => option.value === raw)) {
      next.intervalMs = raw;
      push(
        "intervalMs",
        "adopted",
        labelOf(INTERVAL_OPTIONS, raw),
        raw,
        raw !== current.intervalMs
      );
    } else {
      next.intervalMs = DEFAULT_INTERVAL_MS;
      push(
        "intervalMs",
        "clamped",
        labelOf(INTERVAL_OPTIONS, DEFAULT_INTERVAL_MS),
        raw,
        DEFAULT_INTERVAL_MS !== current.intervalMs
      );
    }
  }

  // 趋势窗口：同上，白名单是 TREND_RANGES
  {
    const raw = source.trendRangeSecs;
    if (raw === undefined) {
      push("trendRangeSecs", "absent", labelOf(TREND_RANGES, current.trendRangeSecs), raw, false);
    } else if (typeof raw !== "number" || !Number.isFinite(raw)) {
      push("trendRangeSecs", "invalid", labelOf(TREND_RANGES, current.trendRangeSecs), raw, false);
    } else if (TREND_RANGES.some((range) => range.value === raw)) {
      next.trendRangeSecs = raw;
      push(
        "trendRangeSecs",
        "adopted",
        labelOf(TREND_RANGES, raw),
        raw,
        raw !== current.trendRangeSecs
      );
    } else {
      next.trendRangeSecs = DEFAULT_TREND_SECS;
      push(
        "trendRangeSecs",
        "clamped",
        labelOf(TREND_RANGES, DEFAULT_TREND_SECS),
        raw,
        DEFAULT_TREND_SECS !== current.trendRangeSecs
      );
    }
  }

  // 页签：白名单来自 App 真实的 tabItems
  {
    const raw = source.activeTab;
    const fallbackTab = tabs.includes(DEFAULT_TAB) ? DEFAULT_TAB : (tabs[0] ?? DEFAULT_TAB);
    if (raw === undefined) {
      push("activeTab", "absent", current.activeTab, raw, false);
    } else if (typeof raw !== "string" || !tabs.includes(raw)) {
      // 与 App 的回落口径一致：认不出的页签回到概览
      next.activeTab = fallbackTab;
      push("activeTab", "clamped", fallbackTab, raw, fallbackTab !== current.activeTab);
    } else {
      next.activeTab = raw;
      push("activeTab", "adopted", raw, raw, raw !== current.activeTab);
    }
  }

  // 告警阈值：整体过 normalizeAlertConfig，与 localStorage 读侧同一条链
  {
    const raw = source.alert;
    const currentAlert = normalizeAlertConfig(current.alert);
    if (raw === undefined) {
      push("alert", "absent", alertText(currentAlert), raw, false);
    } else if (typeof raw !== "object" || raw === null) {
      push("alert", "invalid", alertText(currentAlert), raw, false);
    } else {
      const normalized = normalizeAlertConfig(raw);
      next.alert = normalized;
      push(
        "alert",
        alertWasFolded(raw, normalized) ? "clamped" : "adopted",
        alertText(normalized),
        raw,
        JSON.stringify(normalized) !== JSON.stringify(currentAlert)
      );
    }
  }

  const unknownKeys = Object.keys(source).filter((key) => !PREFS_KEYS.includes(key as PrefsKey));

  return {
    next,
    entries,
    unknownKeys,
    adopted: entries.filter((e) => e.outcome === "adopted").length,
    clamped: entries.filter((e) => e.outcome === "clamped").length,
    kept: entries.filter((e) => e.outcome === "absent" || e.outcome === "invalid").length,
    changed: entries.filter((e) => e.differs).length,
  };
}

/** 建议的导出文件名：带上秒级时间戳，同一目录里重复导出不会互相覆盖成一份无名文件。 */
export function defaultPrefsFileName(ms: number = Date.now()): string {
  const d = new Date(ms);
  const p = (n: number) => String(n).padStart(2, "0");
  return `z-biz-tool-sys-prefs-${d.getFullYear()}${p(d.getMonth() + 1)}${p(d.getDate())}-${p(
    d.getHours()
  )}${p(d.getMinutes())}${p(d.getSeconds())}.json`;
}
