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

export type ProcessSort = "cpu" | "memory" | "pid" | "name";

/**
 * 进程查询：关键字过滤、排序与分页都在后端完成。
 * `total` 是命中总数，因此搜索不再受"只看 CPU 前若干行"限制。
 */
export interface ProcessQuery {
  keyword: string;
  sortBy: ProcessSort;
  desc: boolean;
  offset: number;
  limit: number;
}

/** 点击进程行按需拉取；刻意不含命令行参数与环境变量 */
export interface ProcessDetail {
  pid: number;
  name: string;
  parentPid: number | null;
  exePath: string | null;
  cwd: string | null;
  status: string;
  /** Unix 秒级时间戳，sysinfo 读不到时为 0 */
  startTime: number;
  runTimeSeconds: number;
  cpuUsage: number;
  memoryBytes: number;
  isSelf: boolean;
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

/**
 * 趋势采样点。实时链路与落盘历史共用同一形状（T3-07），
 * 因此前端可以把两者直接拼成一条曲线。
 */
export interface HistoryPoint {
  /** 毫秒时间戳 */
  t: number;
  cpu: number;
  /** 内存使用率百分比，不是字节 */
  memory: number;
  rxBytesPerSec: number;
  txBytesPerSec: number;
}

/**
 * `get_history` 返回：点 + 这批点的真实分辨率与覆盖情况。
 *
 * `bucketSeconds`/`storedPoints`/`oldestMs` 存在的理由：调用方必须能分辨
 * "这 1 小时真有 360 个 10 s 点"和"只有 20 分钟数据、按桶平均出来的点"。
 */
export interface HistoryPage {
  points: HistoryPoint[];
  spanSeconds: number;
  /** 每个点代表的区段长度；等于采样间隔 10 时即未压缩的原始点 */
  bucketSeconds: number;
  /** 该区间内实际落盘的点数（分桶压缩之前） */
  storedPoints: number;
  /** 文件里最早可回溯到的点；无历史时为 null */
  oldestMs: number | null;
  newestMs: number | null;
  /** 解析失败的行数（进程被强杀时尾部半行），只报告不修补 */
  unreadableLines: number;
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
  /** true 表示用户中途取消，categories 只是已扫完部分的结果 */
  cancelled: boolean;
}

/** `sys://cleanup-scan-progress` 每帧载荷（T3-09） */
export interface ScanProgress {
  scannedPaths: number;
  totalPaths: number;
  currentPath: string;
  currentPathFiles: number;
  foundFiles: number;
  foundBytes: number;
  cancelled: boolean;
  done: boolean;
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
  /** 垃圾扫描进度帧，仅 scan_junk_files 运行期间推送 */
  cleanupScanProgress: "sys://cleanup-scan-progress",
} as const;

export const Commands = {
  getStaticInfo: "get_static_info",
  setMonitorConfig: "set_monitor_config",
  startProcessStream: "start_process_stream",
  stopProcessStream: "stop_process_stream",
  setProcessQuery: "set_process_query",
  getMetricsSnapshot: "get_metrics_snapshot",
  /** 读取已落盘的历史趋势点（T3-07）；后端按 10 s 采样、保留 7 天 */
  getHistory: "get_history",
  getProcesses: "get_processes",
  validateKill: "validate_kill",
  killProcess: "kill_process",
  flushDns: "flush_dns_cache",
  scanJunkFiles: "scan_junk_files",
  /** 取消进行中的垃圾扫描，返回 false 表示当前根本没有扫描在跑 */
  cancelJunkScan: "cancel_junk_scan",
  getProcessDetail: "get_process_detail",
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
