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

export type SensorKind = "temperature" | "fan";
/** 温度等级由后端按硬件自己上报的上限算出，前端只做上色（"判定不在前端"这条与告警一致） */
export type SensorSeverity = "ok" | "warning" | "critical" | "unknown";

/** 单个传感器读数（T5-08）。`value` 刻意不是 `number | null`：后端读不出的槽位不会进列表，
 *  所以界面上不存在"把 0 当成缺测"这种误读。 */
export interface ThermalSensor {
  label: string;
  kind: SensorKind;
  value: number;
  /** 传感器**自己**上报的临界值（Linux hwmon 的 `tempN_max`），不是应用阈值 */
  critical: number | null;
  severity: SensorSeverity;
  /** 读数出自哪个内核节点，作为"这块数据从哪来"的凭据 */
  source: string;
}

/** `get_thermal` 载荷：空列表一定带原因，非空列表一定不带原因 */
export interface ThermalReport {
  sensors: ThermalSensor[];
  reason: string | null;
}

/**
 * 系统通知（T5-02）的投递记账。刻意只有"提交/失败"两个数，**没有"已送达"**：
 * macOS 的默认通知后端是在句柄析构时才真正发送、并把错误丢掉的，
 * 所以 `submitted` 只说明"这条交给了操作系统的通知接口"。
 */
export interface NotifyStatus {
  submitted: number;
  failed: number;
  /** 最后一次失败的原文；从没失败过时是 `null`，那不等于"全都弹出来了" */
  lastError: string | null;
  /** 本平台能不能报出投递失败。macOS 为 `false`，界面据此决定要不要挂那句说明 */
  deliveryIsReported: boolean;
}

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

/**
 * 后端 `error.rs` 的构造器集合，两边必须一一对应。
 * 由 `error_codes_match_the_frontend_ipc_contract` 逐条核对（含"后端有构造器但前端没分支"和反向）。
 */
export const AppErrorCode = {
  permissionDenied: "PERMISSION_DENIED",
  notFound: "NOT_FOUND",
  processNotFound: "PID_NOT_FOUND",
  pathDenied: "PATH_DENIED",
  invalidInput: "INVALID_INPUT",
  commandFailed: "COMMAND_FAILED",
  unsupported: "UNSUPPORTED",
} as const;

export interface DnsFlushResult {
  flushed: boolean;
  message: string;
  /** 自动刷新失败时，后端给出的可自行执行的命令（应用内绝不调用 sudo） */
  manualCommand: string | null;
}

/**
 * 告警契约（T5-01）。判定全在后端 `alert.rs`：前端只是配置的生产者和事件消费者，
 * 这样"连续 N 帧"的口径不会因为页面切换 / 前端节流而变形。
 */
export type AlertMetric = "cpu" | "memory" | "disk";
export type AlertLevel = "warning" | "critical";

export interface AlertThresholds {
  warning: number;
  critical: number;
}

/** 阈值配置；`set_alert_config` 返回的是后端夹取后真正生效的值 */
export interface AlertConfig {
  enabled: boolean;
  /** 连续多少帧越限才触发，1 表示第一帧即触发；后端范围 1~60 */
  consecutive: number;
  /** 同一告警的最小重复推送间隔（秒）；后端上限 86400 */
  cooldownSecs: number;
  cpu: AlertThresholds;
  memory: AlertThresholds;
  disk: AlertThresholds;
}

/** `sys://alert` 载荷 */
export interface AlertEvent {
  metric: AlertMetric;
  level: AlertLevel;
  /** 触发帧的实际值（百分比） */
  value: number;
  /** 被越过的那一档阈值（百分比） */
  threshold: number;
  /** 磁盘告警的挂载点；CPU / 内存为 null */
  target: string | null;
  /** 触发时的连续越限帧数 */
  consecutive: number;
  timestampMs: number;
}

/**
 * `get_alert_history` 载荷（T5-03）：已落盘的告警，按时间倒序。
 *
 * `storedEvents` 与 `totalEvents` 必须分开看：前者是"这次窗口里有几条"，
 * 后者是"文件里现存几条"。只给一个数就会让界面把"这条盘只留了 7 天"说成"从来没发生过告警"。
 * `storedEvents` 可能大于 `events.length` —— 后端对**列表**做了截断（只给最近的若干条），
 * 但窗口里的真实条数照常报，界面据此说"另有 N 条未列出"。
 * `oldestMs`/`newestMs` 描述的是**窗口内**的时间戳，窗口为空时是 `null`（不是省略、也不是 0）。
 */
export interface AlertHistoryPage {
  events: AlertEvent[];
  spanSeconds: number;
  storedEvents: number;
  totalEvents: number;
  oldestMs: number | null;
  newestMs: number | null;
  unreadableLines: number;
}

/**
 * 偏好导入/导出（T5-11）。后端 `prefs.rs` 只管落盘边界（路径、体积、原子写、格式身份），
 * 把 `prefs` 当一个不透明对象透传 —— 值的合法性口径仍然只在 `lib/prefs_file.ts` 一处。
 * 因此这里的 `prefs` 是 `Record<string, unknown>`：文件是用户可编辑的，不能假装它一定合契约。
 */
export interface PrefsSnapshot {
  darkMode: boolean;
  intervalMs: number;
  trendRangeSecs: number;
  activeTab: string;
  alert: AlertConfig;
}

/** `export_prefs_file` 载荷：偏好的 5 项，不含任何路径 / 进程 / 主机信息 */
export interface PrefsExportOutcome {
  /** 真正写入的路径（`~` 已展开、符号链接已解析） */
  path: string;
  bytes: number;
  keys: number;
  schemaVersion: number;
}

/** `import_prefs_file` 载荷：只证明"认得出这份文件"，不代表值可用 */
export interface PrefsImportOutcome {
  path: string;
  bytes: number;
  schemaVersion: number;
  exportedAtMs: number | null;
  prefs: Record<string, unknown>;
  /** 文件自带的项数（归一化之前），与"实际采纳了几项"对比才能说清"另有 N 项被忽略" */
  fileKeys: number;
}

/**
 * 诊断 Agent（T5-04 / T5-05 / T5-06）。判定全在后端 `agent.rs`，前端只渲染。
 *
 * 边界写在类型上（04 文档 S4：Agent 只建议、不执行）：
 * `AgentSuggestion` 只有"打开页签""定位进程"两种，没有命令 / 参数 / 路径字段可填；
 * `executed` 恒为 false，前端也不得因为收到回复就调用任何破坏性命令。
 */
export type AgentIntent =
  | "rankProcesses"
  | "diagnoseSlowness"
  | "memoryPressure"
  | "diskSpace"
  | "networkThroughput"
  | "startupItems"
  | "junkFiles"
  | "temperature"
  | "unknown";
export type AgentRankBy = "cpu" | "memory";
export type AgentSeverity = "info" | "warning" | "critical";
export type AgentMetric = "cpu" | "memory" | "disk" | "network" | "process" | "thermal";

export interface AgentFinding {
  metric: AgentMetric;
  /** 已脱敏的人话结论 */
  text: string;
  /** 度量值；后端没有来源时为 null，界面显示 `—` 而不是 0 */
  value: string | null;
  level: AgentSeverity;
}

/** 预定义操作模板的全部种类：`tab` 对应 App.tsx 的页签 key */
export type AgentSuggestion =
  | { kind: "openTab"; tab: string; label: string }
  | { kind: "focusProcess"; pid: number; name: string; label: string };

/** `agent_query` 载荷 */
export interface AgentReply {
  query: string;
  intent: AgentIntent;
  /** 仅 `intent === "rankProcesses"` 时有值 */
  rankBy: AgentRankBy | null;
  /** true 表示这句话被认成"要我代执行"，已拒绝，结论与建议为空 */
  refused: boolean;
  findings: AgentFinding[];
  suggestions: AgentSuggestion[];
  notes: string[];
  executed: boolean;
}

export const MonitorEvent = {
  metrics: "sys://metrics",
  processes: "sys://processes",
  /** 垃圾扫描进度帧，仅 scan_junk_files 运行期间推送 */
  cleanupScanProgress: "sys://cleanup-scan-progress",
  /** 阈值告警（T5-01），后端只在连续超限那一帧推一次，冷却窗口内不重复 */
  alert: "sys://alert",
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
  /** 读取后端真正生效的告警阈值（T5-01） */
  getAlertConfig: "get_alert_config",
  /** 下发告警阈值，返回后端夹取后的生效值 */
  setAlertConfig: "set_alert_config",
  /** 读已落盘的告警历史（T5-03），返回按时间倒序的一页 */
  getAlertHistory: "get_alert_history",
  /** 把当前偏好写成 JSON 文件（T5-11）；路径来自系统保存框，后端只认用户可写位置 */
  exportPrefsFile: "export_prefs_file",
  /** 读回一份偏好文件（T5-11）；只校验格式与来源，值仍由前端白名单归一化 */
  importPrefsFile: "import_prefs_file",
  /** 诊断 Agent 问答（T5-04）；只读，返回结论与待确认的导航建议，永不执行 */
  agentQuery: "agent_query",
  /** 温度/风扇读数（T5-08）；无参数、只读，没有免提权通路的平台返回空列表 + 原因 */
  getThermal: "get_thermal",
  /** 这一次运行里系统通知的投递记账（T5-02）；无参数 */
  notifyStatus: "notify_status",
  /** 投一条文案固定的测试通知（T5-02）；不接收任何参数，避免变成任意文本注入通道 */
  sendTestNotification: "send_test_notification",
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

/** 把 invoke 抛出的值认成 AppError；非结构化错误（无 IPC 通道、网络层抛错）返回 null */
export function asAppError(e: unknown): AppErrorPayload | null {
  if (!e || typeof e !== "object") return null;
  const payload = e as Partial<AppErrorPayload>;
  return typeof payload.code === "string" && typeof payload.message === "string"
    ? (payload as AppErrorPayload)
    : null;
}
