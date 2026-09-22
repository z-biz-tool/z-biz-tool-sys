// 前端与 Rust 后端的 IPC 契约。字段名与后端 `#[serde(rename_all = "camelCase")]` 一一对应。
// 所有字节量一律以「字节」传输，展示层负责格式化。

export type MemoryPressure = "normal" | "warning" | "critical";
export type InterfaceStatus = "up" | "down" | "unknown";

export interface CpuMetrics {
  total: number;
  perCore: number[];
  coreCount: number;
}

export interface MemoryMetrics {
  totalBytes: number;
  usedBytes: number;
  availableBytes: number;
  swapTotalBytes: number;
  swapUsedBytes: number;
  usagePercent: number;
  pressure: MemoryPressure;
}

export interface DiskMetrics {
  name: string;
  mountPoint: string;
  fileSystem: string;
  totalBytes: number;
  usedBytes: number;
  availableBytes: number;
  usagePercent: number;
  readBytesPerSec: number | null;
  writeBytesPerSec: number | null;
  available: boolean;
}

export interface NetworkMetrics {
  interface: string;
  status: InterfaceStatus;
  ipv4: string | null;
  ipv6: string | null;
  mac: string | null;
  rxBytesPerSec: number;
  txBytesPerSec: number;
  totalReceivedBytes: number;
  totalTransmittedBytes: number;
  packetsReceived: number;
  packetsTransmitted: number;
}

/** `sys://metrics` 每帧载荷 */
export interface MetricsSnapshot {
  timestampMs: number;
  uptimeSeconds: number;
  cpu: CpuMetrics;
  memory: MemoryMetrics;
  disks: DiskMetrics[];
  networks: NetworkMetrics[];
}

/** `sys://processes` 载荷 */
export interface ProcessInfo {
  pid: number;
  name: string;
  cpuUsage: number;
  memoryBytes: number;
  threads: number | null;
  userName: string | null;
  parentPid: number | null;
  runTimeSeconds: number | null;
}

export interface ProcessPage {
  total: number;
  items: ProcessInfo[];
  timestampMs: number;
  /** true 表示 CPU 列仍在暖机，数值不可信 */
  warming: boolean;
}

/** 启动时一次性拉取的静态信息 */
export interface StaticInfo {
  hostname: string;
  osName: string;
  osVersion: string;
  kernelVersion: string;
  cpuModel: string;
  coreCount: number;
  totalMemoryBytes: number;
  bootTimeSeconds: number | null;
  appVersion: string;
  platform: string;
  arch: string;
  currentUserName: string;
}

export type RiskLevel = "safe" | "moderate" | "risky";

export interface JunkCategory {
  id: string;
  name: string;
  description: string;
  paths: string[];
  sizeBytes: number;
  fileCount: number;
  riskLevel: RiskLevel;
}

export interface JunkReport {
  totalSizeBytes: number;
  totalFiles: number;
  categories: JunkCategory[];
  scanTimeMs: number;
}

export interface CleanupResult {
  freedBytes: number;
  deletedFiles: number;
  failedFiles: number;
  /** 命中黑名单而整目录跳过的数量 */
  skippedPaths: number;
  errors: string[];
}

export interface LargeFile {
  path: string;
  sizeBytes: number;
  modifiedSeconds: number;
  isDir: boolean;
}

export interface StartupItem {
  id: string;
  name: string;
  command: string;
  source: string;
  enabled: boolean;
  location: string;
}

/** kill 分级：blocked 禁止操作，critical 需逐字输入进程名确认，standard 需对话框确认 */
export type KillRiskLevel = "blocked" | "critical" | "standard";

export interface KillValidation {
  pid: number;
  exists: boolean;
  allowed: boolean;
  riskLevel: KillRiskLevel;
  processName: string;
  userName: string | null;
  memoryBytes: number;
  /** 不允许结束自身进程 */
  isSelf: boolean;
  deniedReason: string | null;
}

/** 后端 AppError 的前端形态 */
export interface AppErrorPayload {
  code: string;
  message: string;
  detail?: string | null;
}

export interface DnsFlushResult {
  flushed: boolean;
  message: string;
  /** 自动刷新失败时，后端给出的可自行执行的命令（应用内绝不调用 sudo） */
  manualCommand: string | null;
}

export const MonitorEvent = {
  metrics: "sys://metrics",
  processes: "sys://processes",
  cleanupProgress: "sys://cleanup-progress",
} as const;

export const Commands = {
  getStaticInfo: "get_static_info",
  setMonitorConfig: "set_monitor_config",
  startProcessStream: "start_process_stream",
  stopProcessStream: "stop_process_stream",
  getMetricsSnapshot: "get_metrics_snapshot",
  getProcesses: "get_processes",
  validateKill: "validate_kill",
  killProcess: "kill_process",
  flushDns: "flush_dns_cache",
  scanJunkFiles: "scan_junk_files",
  cleanupJunkFiles: "cleanup_junk_files",
  findLargeFiles: "find_large_files_cmd",
  getStartupItems: "get_startup_items_cmd",
} as const;

/** 把 invoke/listen 抛出的任意值归一化为可读文案 */
export function describeError(e: unknown): string {
  if (e && typeof e === "object") {
    const payload = e as Partial<AppErrorPayload>;
    if (typeof payload.message === "string") {
      return payload.detail ? `${payload.message}（${payload.detail}）` : payload.message;
    }
  }
  return String(e);
}

export function isAppErrorCode(e: unknown, code: string): boolean {
  return (
    !!e && typeof e === "object" && (e as Partial<AppErrorPayload>).code === code
  );
}
