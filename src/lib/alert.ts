import type { AlertConfig, AlertEvent, AlertLevel, AlertMetric, AlertThresholds } from "../ipc_contract";

/**
 * 告警阈值的默认值与前端侧归一化（T5-01）。
 *
 * 判定逻辑全在后端 `alert.rs`，这里只做两件事：给界面一份初始配置，以及把 localStorage
 * 里的陌生值折回合法区间 —— 与 `usePrefs` 其它项同一口径（曾有一个不认识的持久化值把界面打成空白页）。
 * 夹取规则与后端 `AlertConfig::clamped()` 逐条对齐，界面上显示的值必须就是生效的值。
 */
export const DEFAULT_ALERT_CONFIG: AlertConfig = {
  enabled: true,
  consecutive: 3,
  cooldownSecs: 60,
  // 02 文档 F6 只钉了 warning 档（CPU 80 / 内存 85 / 磁盘 90），critical 取 95
  cpu: { warning: 80, critical: 95 },
  memory: { warning: 85, critical: 95 },
  disk: { warning: 90, critical: 95 },
};

export const ALERT_METRIC_LABELS: Record<AlertMetric, string> = {
  cpu: "CPU 使用率",
  memory: "内存使用率",
  disk: "磁盘使用率",
};

export const ALERT_LEVEL_LABELS: Record<AlertLevel, string> = {
  warning: "警告",
  critical: "严重",
};

export const ALERT_METRIC_ORDER: AlertMetric[] = ["cpu", "memory", "disk"];

const num = (value: unknown, fallback: number, min: number, max: number) =>
  typeof value === "number" && Number.isFinite(value) ? Math.min(max, Math.max(min, value)) : fallback;

function normalizeThresholds(raw: unknown, fallback: AlertThresholds): AlertThresholds {
  const source = (raw ?? {}) as Partial<AlertThresholds>;
  const warning = num(source.warning, fallback.warning, 1, 100);
  // critical 低于 warning 会让一帧同时"越过两档"，后端按 warning 收敛，前端必须显示同一个结果
  const critical = Math.max(num(source.critical, fallback.critical, 1, 100), warning);
  return { warning, critical };
}

export function normalizeAlertConfig(raw: unknown): AlertConfig {
  const source = (raw ?? {}) as Partial<AlertConfig>;
  const known = ALERT_METRIC_ORDER.reduce(
    (acc, metric) => {
      acc[metric] = normalizeThresholds(source[metric], DEFAULT_ALERT_CONFIG[metric]);
      return acc;
    },
    {} as Record<AlertMetric, AlertThresholds>
  );
  return {
    enabled: typeof source.enabled === "boolean" ? source.enabled : DEFAULT_ALERT_CONFIG.enabled,
    consecutive: Math.round(num(source.consecutive, DEFAULT_ALERT_CONFIG.consecutive, 1, 60)),
    cooldownSecs: Math.round(num(source.cooldownSecs, DEFAULT_ALERT_CONFIG.cooldownSecs, 0, 86_400)),
    cpu: known.cpu,
    memory: known.memory,
    disk: known.disk,
  };
}

export function sameAlertConfig(a: AlertConfig, b: AlertConfig): boolean {
  return JSON.stringify(normalizeAlertConfig(a)) === JSON.stringify(normalizeAlertConfig(b));
}

/**
 * 告警历史的查看窗口。上界是后端的保留窗口（30 天），默认 7 天与 `DEFAULT_ALERT_SPAN_SECS` 对齐；
 * 超出上界后端会夹取，界面上不能装作能选更长。
 */
export const ALERT_HISTORY_SPANS = [
  { label: "1 小时", value: 3600 },
  { label: "24 小时", value: 86_400 },
  { label: "7 天", value: 604_800 },
];

export const DEFAULT_ALERT_HISTORY_SPAN = 604_800;

export const ALERT_LEVEL_COLORS: Record<AlertLevel, string> = {
  warning: "#fa8c16",
  critical: "#f5222d",
};

/**
 * 同一条告警在"会话内推送"与"落盘回读"两份里的身份。
 *
 * 不含 `target`：磁盘每帧只按最满的那一块判定，同一 (指标, 档位, 毫秒) 不会有两条不同分区，
 * 而落盘那份的 target 是脱敏过的，把进 key 会让同一条告警裂成两行。
 */
export const alertIdentity = (event: AlertEvent) =>
  `${event.metric}|${event.level}|${event.timestampMs}`;

/**
 * 合并本次会话与落盘回填的告警，倒序返回（T5-03）。
 *
 * 会话那份**覆盖**落盘那份：它是后端直接推的，挂载点没脱敏，界面上要能回答"是哪块盘"。
 * 这里不补任何时间戳未知的记录 —— 后端给不出 oldestMs 时就是给不出。
 */
export function mergeAlertEvents(session: AlertEvent[], persisted: AlertEvent[]): AlertEvent[] {
  const merged = new Map<string, AlertEvent>();
  for (const event of persisted) merged.set(alertIdentity(event), event);
  for (const event of session) merged.set(alertIdentity(event), event);
  return [...merged.values()].sort((a, b) => b.timestampMs - a.timestampMs);
}

/** 告警文案：说清"哪个指标、连续几帧、越了哪一档、现在是多少"，磁盘还要说哪个分区。 */
export function alertText(event: AlertEvent): string {
  const metric = ALERT_METRIC_LABELS[event.metric] ?? event.metric;
  const target = event.target ? `（${event.target}）` : "";
  const level = ALERT_LEVEL_LABELS[event.level] ?? event.level;
  return `${metric}${target} 连续 ${event.consecutive} 帧越过${level}阈值：${event.value.toFixed(1)} % ≥ ${event.threshold.toFixed(0)} %`;
}
