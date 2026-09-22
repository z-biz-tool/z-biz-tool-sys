import type { ProcessSort } from "../ipc_contract";

export const brandGradient = "linear-gradient(135deg, #667eea 0%, #764ba2 100%)";
export const cardBgGradient =
  "linear-gradient(135deg, rgba(102,126,234,0.04) 0%, rgba(118,75,162,0.04) 100%)";

export const gradientText = {
  background: brandGradient,
  WebkitBackgroundClip: "text",
  WebkitTextFillColor: "transparent",
} as const;

export const monitorCardStyle = {
  borderRadius: 12,
  background: cardBgGradient,
  transition: "all 0.3s cubic-bezier(0.4, 0, 0.2, 1)",
};

/** 用户偏好存储前缀（doc/优化方案/05 兼容性承诺约定的 key 命名） */
export const PREF = "z-biz-sys:v1:";

export const INTERVAL_OPTIONS = [
  { value: 500, label: "0.5 秒" },
  { value: 1000, label: "1 秒" },
  { value: 2000, label: "2 秒" },
  { value: 5000, label: "5 秒" },
];

/** 进程表每页行数；分页由后端执行 */
export const PROCESS_PAGE_SIZE = 20;

export const PROCESS_SORT_LABELS: Record<ProcessSort, string> = {
  cpu: "CPU",
  memory: "内存",
  pid: "PID",
  name: "进程名",
};

export const RISK_LABELS: Record<string, string> = {
  safe: "安全",
  moderate: "中等",
  risky: "高风险",
};

export function riskColor(level: string): string {
  if (level === "safe") return "green";
  if (level === "moderate") return "orange";
  if (level === "risky") return "red";
  return "default";
}
