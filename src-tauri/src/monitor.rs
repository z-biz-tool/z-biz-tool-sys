use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use sysinfo::{
    CpuRefreshKind, Disks, MemoryRefreshKind, Networks, ProcessRefreshKind, ProcessesToUpdate,
    RefreshKind, System, UpdateKind,
};
use tauri::{AppHandle, Emitter};

pub const METRICS_EVENT: &str = "sys://metrics";
pub const PROCESSES_EVENT: &str = "sys://processes";

const MIN_INTERVAL_MS: u64 = 200;
const MAX_PROCESSES_PER_PAGE: usize = 300;
const DEFAULT_PROCESS_PAGE_SIZE: usize = 50;
/// 进程表在此时间内视为可信，避免每次 kill 校验都全量枚举。
const PROCESS_CACHE_TTL: Duration = Duration::from_secs(10);
/// 一次性采集时两次采样之间的间隔：太短会让空闲进程算成 0，太长拖着 IPC。
const WARM_SAMPLE_MS: u64 = 260;

// ==================== 契约数据类型 ====================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryPressure {
    Normal,
    Warning,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum InterfaceStatus {
    Up,
    Down,
    Unknown,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CpuMetrics {
    pub total: f64,
    pub per_core: Vec<f64>,
    pub core_count: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryMetrics {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub available_bytes: u64,
    pub swap_total_bytes: u64,
    pub swap_used_bytes: u64,
    pub usage_percent: f64,
    pub pressure: MemoryPressure,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskMetrics {
    pub name: String,
    pub mount_point: String,
    pub file_system: String,
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub available_bytes: u64,
    pub usage_percent: f64,
    pub read_bytes_per_sec: Option<f64>,
    pub write_bytes_per_sec: Option<f64>,
    pub available: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkMetrics {
    pub interface: String,
    pub status: InterfaceStatus,
    pub ipv4: Option<String>,
    pub ipv6: Option<String>,
    pub mac: Option<String>,
    pub rx_bytes_per_sec: f64,
    pub tx_bytes_per_sec: f64,
    pub total_received_bytes: u64,
    pub total_transmitted_bytes: u64,
    pub packets_received: u64,
    pub packets_transmitted: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetricsSnapshot {
    pub timestamp_ms: u64,
    pub uptime_seconds: u64,
    pub cpu: CpuMetrics,
    pub memory: MemoryMetrics,
    pub disks: Vec<DiskMetrics>,
    pub networks: Vec<NetworkMetrics>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
    pub cpu_usage: f64,
    pub memory_bytes: u64,
    pub threads: Option<u64>,
    pub user_name: Option<String>,
    pub parent_pid: Option<u32>,
    pub run_time_seconds: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessPage {
    pub total: usize,
    pub items: Vec<ProcessInfo>,
    pub timestamp_ms: u64,
    /// CPU 列需两次采样差值，暖机帧为 true，此时 cpuUsage 不可信。
    pub warming: bool,
}

/// 进程表排序键。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProcessSort {
    #[default]
    Cpu,
    Memory,
    Pid,
    Name,
}

/// 进程查询：过滤/排序/分页都在后端做，前端只拿到当前页。
/// 否则搜索只能在 CPU 头部若干行里做，冷进程根本搜不到。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProcessQuery {
    pub keyword: String,
    pub sort_by: ProcessSort,
    pub desc: bool,
    pub offset: usize,
    pub limit: usize,
}

impl Default for ProcessQuery {
    fn default() -> Self {
        Self {
            keyword: String::new(),
            sort_by: ProcessSort::default(),
            desc: true,
            offset: 0,
            limit: DEFAULT_PROCESS_PAGE_SIZE,
        }
    }
}

impl ProcessQuery {
    /// 归一化外部输入：关键字去空白转小写，分页规模限制在安全区间。
    pub fn normalized(&self) -> Self {
        Self {
            keyword: self.keyword.trim().to_lowercase(),
            sort_by: self.sort_by,
            desc: self.desc,
            offset: self.offset,
            limit: self.limit.clamp(1, MAX_PROCESSES_PER_PAGE),
        }
    }

    fn matches(&self, proc: &ProcessInfo) -> bool {
        if self.keyword.is_empty() {
            return true;
        }
        proc.name.to_lowercase().contains(&self.keyword)
            || proc.pid.to_string() == self.keyword
    }

    fn compare(&self, a: &ProcessInfo, b: &ProcessInfo) -> std::cmp::Ordering {
        use ProcessSort::*;
        // 统一按升序比较，方向由 desc 一次翻转，避免每个分支各写一遍。
        let ord = match self.sort_by {
            Cpu => a
                .cpu_usage
                .partial_cmp(&b.cpu_usage)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.memory_bytes.cmp(&b.memory_bytes)),
            Memory => a
                .memory_bytes
                .cmp(&b.memory_bytes)
                .then(a.cpu_usage.total_cmp(&b.cpu_usage)),
            Pid => a.pid.cmp(&b.pid),
            Name => a
                .name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then(a.pid.cmp(&b.pid)),
        };
        if self.desc {
            ord.reverse()
        } else {
            ord
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StaticInfo {
    pub hostname: String,
    pub os_name: String,
    pub os_version: String,
    pub kernel_version: String,
    pub cpu_model: String,
    pub core_count: usize,
    pub total_memory_bytes: u64,
    pub boot_time_seconds: u64,
    pub app_version: String,
    pub platform: String,
    pub arch: String,
    pub current_user_name: Option<String>,
}

/// 对外暴露的精简进程视图，供 safety 层复用。
pub struct ProcessSummary {
    pub name: String,
    pub memory_bytes: u64,
}

// ==================== 采集配置 ====================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct MonitorConfig {
    pub interval_ms: u64,
    pub process_interval_ms: u64,
    pub disk_interval_ms: u64,
    pub paused: bool,
}

impl Default for MonitorConfig {
    fn default() -> Self {
        Self {
            // sysinfo 的 CPU 采样要求 >= 200ms，1s 与活动监视器一致。
            interval_ms: 1000,
            process_interval_ms: 3000,
            disk_interval_ms: 10000,
            paused: false,
        }
    }
}

impl MonitorConfig {
    pub fn clamped(self) -> Self {
        Self {
            interval_ms: self.interval_ms.clamp(MIN_INTERVAL_MS, 60_000),
            process_interval_ms: self.process_interval_ms.clamp(1_000, 300_000),
            disk_interval_ms: self.disk_interval_ms.clamp(1_000, 300_000),
            paused: self.paused,
        }
    }
}

// ==================== 采集器 ====================

/// 长期存活的采集器：`System`/`Networks`/`Disks` 各一份，靠连续刷新产生增量。
pub struct Collector {
    sys: System,
    networks: Networks,
    disks: Disks,
    last_cpu_sample: Instant,
    last_network_sample: Instant,
    last_disk_sample: Instant,
    /// mount_point -> (读 B/s, 写 B/s)。只在真正刷新磁盘的那一帧更新，
    /// 因为 sysinfo 的 `Disk::usage()` 是"自上次 refresh 的增量"，重复读取会让
    /// 分子冻结、分母继续长大，把速率算成越来越小的假值。
    disk_rates: HashMap<String, (Option<f64>, Option<f64>)>,
}

impl Collector {
    pub fn new() -> Self {
        let mut sys = System::new_with_specifics(
            RefreshKind::nothing()
                .with_cpu(CpuRefreshKind::everything())
                .with_memory(MemoryRefreshKind::everything()),
        );
        // 型号与核心数只在首次采样时填充，之后 refresh_cpu_usage 只更新百分比。
        sys.refresh_cpu_all();

        Self {
            sys,
            networks: Networks::new_with_refreshed_list(),
            // `new_with_refreshed_list` 已按 `DiskRefreshKind::everything()` 取过 IOKit 计数，
            // 因此第一次 refresh 的增量就是真实区间吞吐，不会把开机至今的总量当成一帧。
            disks: Disks::new_with_refreshed_list(),
            last_cpu_sample: Instant::now(),
            last_network_sample: Instant::now(),
            last_disk_sample: Instant::now(),
            disk_rates: HashMap::new(),
        }
    }

    pub fn refresh_cpu(&mut self) {
        self.sys.refresh_cpu_usage();
        self.last_cpu_sample = Instant::now();
    }

    pub fn cpu_metrics(&self) -> CpuMetrics {
        let per_core: Vec<f64> = self.sys.cpus().iter().map(|c| c.cpu_usage() as f64).collect();
        let total = if per_core.is_empty() {
            self.sys.global_cpu_usage() as f64
        } else {
            per_core.iter().sum::<f64>() / per_core.len() as f64
        };
        CpuMetrics {
            total: round2(total),
            core_count: per_core.len(),
            per_core: per_core.into_iter().map(round2).collect(),
        }
    }

    pub fn memory_metrics(&mut self) -> MemoryMetrics {
        self.sys.refresh_memory();
        let total = self.sys.total_memory();
        let used = self.sys.used_memory();
        // sysinfo 0.30 在 macOS 上按 free+inactive+purgeable-compressor 计算可用量，
        // 压缩内存吃紧时会 saturating_sub 归零；未使用的部分至少是可回收的下限。
        let available = self.sys.available_memory().max(total.saturating_sub(used));
        let usage_percent = if total > 0 {
            (used as f64 / total as f64) * 100.0
        } else {
            0.0
        };
        MemoryMetrics {
            total_bytes: total,
            used_bytes: used,
            available_bytes: available,
            swap_total_bytes: self.sys.total_swap(),
            swap_used_bytes: self.sys.used_swap(),
            usage_percent: round2(usage_percent),
            pressure: pressure_for(usage_percent),
        }
    }

    pub fn network_metrics(&mut self) -> Vec<NetworkMetrics> {
        self.networks.refresh(true);
        let elapsed = self.last_network_sample.elapsed().as_secs_f64().max(0.001);
        self.last_network_sample = Instant::now();

        let addresses: HashMap<String, crate::netinfo::InterfaceAddress> = crate::netinfo::list_interfaces()
            .into_iter()
            .map(|iface| (iface.name.clone(), iface))
            .collect();

        let mut out = Vec::new();
        for (name, data) in self.networks.iter() {
            let addr = addresses.get(name);
            let received = data.received() as f64 / elapsed;
            let transmitted = data.transmitted() as f64 / elapsed;
            out.push(NetworkMetrics {
                interface: name.clone(),
                status: match addr.map(|a| (a.up, a.loopback)) {
                    Some((true, _)) => InterfaceStatus::Up,
                    Some((false, _)) => InterfaceStatus::Down,
                    None => InterfaceStatus::Unknown,
                },
                ipv4: addr.and_then(|a| a.ipv4.clone()),
                ipv6: addr.and_then(|a| a.ipv6.clone()),
                mac: addr.and_then(|a| a.mac.clone()),
                rx_bytes_per_sec: round2(received),
                tx_bytes_per_sec: round2(transmitted),
                total_received_bytes: data.total_received(),
                total_transmitted_bytes: data.total_transmitted(),
                packets_received: data.packets_received(),
                packets_transmitted: data.packets_transmitted(),
            });
        }
        out.sort_by(|a, b| a.interface.cmp(&b.interface));
        out
    }

    /// 磁盘容量按 `disk_interval_ms` 慢刷；I/O 速率是该刷新窗口的均值，
    /// 窗口内各帧沿用同一份缓存值（速率本身就是"这一段平均多少 B/s"，无需每帧重算）。
    pub fn disk_metrics(&mut self, disk_interval: Duration) -> Vec<DiskMetrics> {
        if self.last_disk_sample.elapsed() >= disk_interval {
            let window = self.last_disk_sample.elapsed().as_secs_f64().max(0.001);
            self.disks.refresh(true);
            self.disk_rates = self
                .disks
                .iter()
                .map(|disk| {
                    let usage = disk.usage();
                    let rate = if usage.total_read_bytes == 0 && usage.total_written_bytes == 0 {
                        // 平台拿不到块设备计数（累计量恒为 0）时不给值；
                        // 反过来，有累计量而本窗口增量为 0 是真·空闲，应当显示 0 而不是"—"。
                        (None, None)
                    } else {
                        (
                            Some(round2(usage.read_bytes as f64 / window)),
                            Some(round2(usage.written_bytes as f64 / window)),
                        )
                    };
                    (disk.mount_point().to_string_lossy().into_owned(), rate)
                })
                .collect();
            self.last_disk_sample = Instant::now();
        }

        let mut out = Vec::new();
        for disk in self.disks.iter() {
            let total = disk.total_space();
            let available = disk.available_space();
            let used = total.saturating_sub(available);
            let key = disk.mount_point().to_string_lossy().into_owned();

            // 首个刷新窗口到来之前没有任何增量可说，保持 None。
            let (read_rate, write_rate) = self.disk_rates.get(&key).copied().unwrap_or((None, None));

            out.push(DiskMetrics {
                name: disk.name().to_string_lossy().into_owned(),
                mount_point: key,
                file_system: disk.file_system().to_string_lossy().into_owned(),
                available: total > 0,
                total_bytes: total,
                used_bytes: used,
                available_bytes: available,
                usage_percent: if total > 0 {
                    round2(used as f64 / total as f64 * 100.0)
                } else {
                    0.0
                },
                read_bytes_per_sec: read_rate,
                write_bytes_per_sec: write_rate,
            });
        }
        out.sort_by_key(|b| std::cmp::Reverse(b.total_bytes));
        out
    }

    pub fn snapshot(&mut self, disk_interval: Duration) -> MetricsSnapshot {
        let cpu = {
            self.refresh_cpu();
            self.cpu_metrics()
        };
        let memory = self.memory_metrics();
        let networks = self.network_metrics();
        let disks = self.disk_metrics(disk_interval);

        MetricsSnapshot {
            timestamp_ms: epoch_ms(),
            uptime_seconds: System::uptime(),
            cpu,
            memory,
            disks,
            networks,
        }
    }
}

/// 启动时一次性采集，不参与定时循环。
pub fn collect_static_info() -> StaticInfo {
    {
        let mut sys = System::new_with_specifics(
            RefreshKind::nothing()
                .with_cpu(CpuRefreshKind::everything())
                .with_memory(MemoryRefreshKind::everything()),
        );
        sys.refresh_cpu_all();
        sys.refresh_memory();
        let uptime = System::uptime();
        StaticInfo {
            hostname: System::host_name().unwrap_or_else(|| "unknown".into()),
            os_name: System::name().unwrap_or_else(|| "unknown".into()),
            os_version: System::long_os_version()
                .or_else(System::os_version)
                .unwrap_or_else(|| "unknown".into()),
            kernel_version: System::kernel_version().unwrap_or_else(|| "unknown".into()),
            cpu_model: sys
                .cpus()
                .first()
                .map(|c| c.brand().to_string())
                .unwrap_or_else(|| "unknown".into()),
            core_count: sys.cpus().len(),
            total_memory_bytes: sys.total_memory(),
            boot_time_seconds: epoch_secs().saturating_sub(uptime),
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            platform: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            current_user_name: crate::safety::current_user_name(),
        }
    }
}

impl Default for Collector {
    fn default() -> Self {
        Self::new()
    }
}

// ==================== 进程采集 ====================

/// 进程表/排行榜的采样项：只要内存和 CPU 占用。
///
/// 刻意不取 `cmd` / `environ` / `root` —— `ProcessRefreshKind::everything()` 会把命令行参数与
/// 环境变量一并读进来（sysinfo 0.33.1 `common/system.rs` 的 `everything()` 里 `cmd`/`environ`
/// 都是 `UpdateKind::OnlyIfNotSet`），直接违反 04 文档"不采集命令行参数与环境变量"那条口径。
/// 也不取 `exe` / `cwd`：3 s 一次的进程流用不到路径，全量枚举时能省掉逐进程的路径读取。
/// 连 sysinfo 自己的默认口径都不要 —— `refresh_processes()` 等价于
/// `nothing().with_memory().with_cpu().with_disk_usage().with_exe(OnlyIfNotSet)`，多读磁盘 I/O
/// 与可执行路径，而且那份口径由依赖决定、升级就会变；只有把要读的东西逐条列出来，
/// "不采集路径/参数/环境"才是可证的，而不是碰巧成立。
/// 公开是给 `commands.rs` 的兜底路径用 —— 任何走 `refresh_processes()` 的地方都会退回宽口径。
pub fn process_refresh_kind() -> ProcessRefreshKind {
    ProcessRefreshKind::nothing().with_memory().with_cpu()
}

/// 进程详情一次性读取的采样项：抽屉里要展示可执行路径和工作目录，所以在这两项上放宽，
/// `cmd` / `environ` 仍然一律不读。
fn detail_refresh_kind() -> ProcessRefreshKind {
    process_refresh_kind()
        .with_exe(UpdateKind::Always)
        .with_cwd(UpdateKind::Always)
}

/// 进程枚举独立于指标采集：枚举成本高，单独低频循环。
///
/// H-06：sysinfo 的进程 CPU 是**跨采样差值**，一个冷 `System` 需要三次触碰才有数
/// （建表 → 填 `old_*` 基准 → 取差值）。实测：`System::new()` 每 300 ms 刷三次 =
/// `[0.0, 0.0, 113.2]`，而 `System::new_with_specifics(带进程项)` 的构造本身就完成了
/// "建表"，同样三次 = `[0.0, 105.6, 110.3]`。此前一次性路径（`get_processes` 兜底、
/// 进程详情、Agent 排行榜）只 `System::new()` + 刷一两次，于是 CPU 恒为 0；
/// 排序断言 `>=` 在全 0 上照样成立，所以旧测试一个都没抓到。
/// 用这个构造器 + 后续两次带间隔的采样即可拿到真实差值。
pub fn process_system() -> System {
    System::new_with_specifics(
        RefreshKind::nothing().with_processes(process_refresh_kind()),
    )
}

/// 一次性采集的**唯一**正确姿势：建表（构造）→ 填基准 → 留出间隔 → 取差值。
/// 三步收在一个函数里，免得每个调用方各自排、排错就成了全 0（H-06）。
pub fn collect_processes_warmed(raw_query: &ProcessQuery) -> ProcessPage {
    let mut sys = process_system();
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, process_refresh_kind());
    let started = Instant::now();
    while started.elapsed() < Duration::from_millis(WARM_SAMPLE_MS) {
        std::thread::sleep(Duration::from_millis(20));
    }
    collect_processes(&mut sys, true, raw_query)
}

/// 全机器排行榜前 N（Agent 用）：走完整三步暖机，不带关键字、不受用户分页影响。
/// 刻意不复用进程表的缓存页 —— 那一页带着用户当前的关键字和分页，
/// 拿它排行会把"在当前这 30 条里最高"说成"整台机器最高"。
pub fn collect_top(by: ProcessSort, limit: usize) -> Vec<ProcessInfo> {
    let query = ProcessQuery {
        keyword: String::new(),
        sort_by: by,
        desc: true,
        offset: 0,
        limit,
    };
    collect_processes_warmed(&query).items
}

pub fn collect_processes(sys: &mut System, warmed: bool, raw_query: &ProcessQuery) -> ProcessPage {
    let query = raw_query.normalized();
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, process_refresh_kind());

    let mut rows: Vec<ProcessInfo> = sys
        .processes()
        .iter()
        .map(|(pid, proc_info)| ProcessInfo {
            pid: pid.as_u32(),
            name: {
                let raw = proc_info.name();
                if raw.is_empty() {
                    "(unknown)".to_string()
                } else {
                    raw.to_string_lossy().into_owned()
                }
            },
            cpu_usage: round2(proc_info.cpu_usage() as f64),
            memory_bytes: proc_info.memory(),
            threads: None,
            // sysinfo 0.30 不提供跨平台的属主名，交由 OS 权限判定兜底。
            user_name: None,
            parent_pid: proc_info.parent().map(|p| p.as_u32()),
            run_time_seconds: Some(proc_info.run_time()),
        })
        .filter(|p| query.matches(p))
        .collect();

    rows.sort_by(|a, b| query.compare(a, b));

    let total = rows.len();
    let start = query.offset.min(total);
    let end = start.saturating_add(query.limit).min(total);

    ProcessPage {
        total,
        items: rows[start..end].to_vec(),
        timestamp_ms: epoch_ms(),
        warming: !warmed,
    }
}

/// 查单个进程；优先命中进程流缓存，未命中时做一次全量枚举。
pub fn lookup_process(pid: u32) -> Option<ProcessSummary> {
    if let Some(service) = service_handle() {
        if let Some(found) = service.cached_process(pid) {
            return Some(found);
        }
    }

    // 走窄口径而不是 `refresh_processes()`：后者会逐进程读 exe 路径，这条兜底路径用不到。
    let mut sys = process_system();
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, process_refresh_kind());
    let key = sysinfo::Pid::from_u32(pid);
    sys.processes().get(&key).map(|p| ProcessSummary {
        name: {
            let raw = p.name();
            if raw.is_empty() {
                "(unknown)".to_string()
            } else {
                raw.to_string_lossy().into_owned()
            }
        },
        memory_bytes: p.memory(),
    })
}

/// 单个进程的详情：可执行路径 / 工作目录 / 父进程 / 启动时刻，供点击进程行后展示。
/// 刻意不含命令行参数与环境变量（见 04 数据安全约束）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessDetail {
    pub pid: u32,
    pub name: String,
    pub parent_pid: Option<u32>,
    pub exe_path: Option<String>,
    pub cwd: Option<String>,
    pub status: String,
    /// Unix 时间戳（秒），sysinfo 在权限不足时为 0。
    pub start_time: u64,
    pub run_time_seconds: u64,
    pub cpu_usage: f64,
    pub memory_bytes: u64,
    pub is_self: bool,
}

fn non_empty_path(p: Option<&std::path::Path>) -> Option<String> {
    p.filter(|path| !path.as_os_str().is_empty())
        .map(|path| path.to_string_lossy().into_owned())
}

/// 详情是一次性读取，不进 3s 事件流：全量枚举一次 ~700 进程约几十毫秒，只在点击时发生。
pub fn collect_process_detail(pid: u32) -> Option<ProcessDetail> {
    // 详情口径带 exe / cwd（抽屉要展示这两项），仍然不带 cmd / environ。
    let mut sys = System::new_with_specifics(
        RefreshKind::nothing().with_processes(detail_refresh_kind()),
    );
    // H-06：构造只完成"建表"，CPU 还要一次填基准、一次取差值 —— 只采一次的话抽屉里是 0。
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, detail_refresh_kind());
    let started = Instant::now();
    while started.elapsed() < Duration::from_millis(WARM_SAMPLE_MS) {
        std::thread::sleep(Duration::from_millis(20));
    }
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, detail_refresh_kind());
    let proc_info = sys.processes().get(&sysinfo::Pid::from_u32(pid))?;
    let raw_name = proc_info.name();
    Some(ProcessDetail {
        pid,
        name: if raw_name.is_empty() {
            "(unknown)".to_string()
        } else {
            raw_name.to_string_lossy().into_owned()
        },
        parent_pid: proc_info.parent().map(|p| p.as_u32()),
        exe_path: non_empty_path(proc_info.exe()),
        cwd: non_empty_path(proc_info.cwd()),
        status: format!("{:?}", proc_info.status()),
        start_time: proc_info.start_time(),
        run_time_seconds: proc_info.run_time(),
        cpu_usage: round2(proc_info.cpu_usage() as f64),
        memory_bytes: proc_info.memory(),
        is_self: pid == std::process::id(),
    })
}

// ==================== 服务 ====================

/// Tauri managed state：唯一采集器 + 最新帧缓存。
pub struct MonitorService {
    config: Arc<RwLock<MonitorConfig>>,
    latest: Arc<RwLock<Option<MetricsSnapshot>>>,
    latest_processes: Arc<RwLock<Option<ProcessPage>>>,
    process_stream: Arc<AtomicBool>,
    process_query: Arc<RwLock<ProcessQuery>>,
    process_query_gen: Arc<AtomicUsize>,
    /// 缓存帧是在哪个 `process_query_gen` 值下产出的，用于判断缓存是否已落后于查询。
    process_page_gen: Arc<AtomicUsize>,
}

static SERVICE: std::sync::OnceLock<RwLock<Option<MonitorService>>> = std::sync::OnceLock::new();

fn service_handle() -> Option<MonitorService> {
    SERVICE
        .get_or_init(|| RwLock::new(None))
        .read()
        .ok()
        .and_then(|g| g.clone())
}

impl Clone for MonitorService {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            latest: self.latest.clone(),
            latest_processes: self.latest_processes.clone(),
            process_stream: self.process_stream.clone(),
            process_query: self.process_query.clone(),
            process_query_gen: self.process_query_gen.clone(),
            process_page_gen: self.process_page_gen.clone(),
        }
    }
}

impl MonitorService {
    pub fn new() -> Self {
        Self {
            config: Arc::new(RwLock::new(MonitorConfig::default())),
            latest: Arc::new(RwLock::new(None)),
            latest_processes: Arc::new(RwLock::new(None)),
            process_stream: Arc::new(AtomicBool::new(false)),
            process_query: Arc::new(RwLock::new(ProcessQuery::default())),
            process_query_gen: Arc::new(AtomicUsize::new(0)),
            process_page_gen: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// 注册为 managed state 前调用一次，使 `lookup_process` 能命中缓存。
    pub fn publish(&self) {
        if let Ok(mut guard) = SERVICE.get_or_init(|| RwLock::new(None)).write() {
            *guard = Some(self.clone());
        }
    }

    pub fn set_config(&self, config: MonitorConfig) -> MonitorConfig {
        let clamped = config.clamped();
        if let Ok(mut guard) = self.config.write() {
            *guard = clamped.clone();
        }
        clamped
    }

    pub fn latest_metrics(&self) -> Option<MetricsSnapshot> {
        self.latest.read().ok().and_then(|g| g.clone())
    }

    pub fn latest_processes(&self) -> Option<ProcessPage> {
        self.latest_processes.read().ok().and_then(|g| g.clone())
    }

    /// 缓存帧是否已反映当前查询；落后时调用方应自行重采集，否则 `get_processes` 会返回上一次的关键字/分页。
    pub fn processes_match_query(&self) -> bool {
        self.process_page_gen.load(Ordering::Relaxed) >= self.process_query_gen.load(Ordering::Relaxed)
    }

    pub fn set_process_stream(&self, enabled: bool) {
        self.process_stream.store(enabled, Ordering::Relaxed);
    }

    pub fn process_stream_enabled(&self) -> bool {
        self.process_stream.load(Ordering::Relaxed)
    }

    /// 设置进程查询（关键字/排序/分页），返回归一化后真正生效的值。
    pub fn set_process_query(&self, query: ProcessQuery) -> ProcessQuery {
        let normalized = query.normalized();
        let changed = self
            .process_query
            .read()
            .map(|guard| *guard != normalized)
            .unwrap_or(true);
        if let Ok(mut guard) = self.process_query.write() {
            *guard = normalized.clone();
        }
        if changed {
            self.process_query_gen.fetch_add(1, Ordering::Relaxed);
            #[cfg(debug_assertions)]
            eprintln!(
                "[monitor] 进程查询变更: 关键字={:?} 排序={:?} desc={} offset={} limit={}",
                normalized.keyword, normalized.sort_by, normalized.desc, normalized.offset, normalized.limit
            );
        }
        normalized
    }

    pub fn current_process_query(&self) -> ProcessQuery {
        self.process_query
            .read()
            .map(|g| g.clone())
            .unwrap_or_default()
    }

    fn cached_process(&self, pid: u32) -> Option<ProcessSummary> {
        let guard = self.latest_processes.read().ok()?;
        let page = guard.as_ref()?;
        if epoch_ms().saturating_sub(page.timestamp_ms) > PROCESS_CACHE_TTL.as_millis() as u64 {
            return None;
        }
        page.items.iter().find(|p| p.pid == pid).map(|p| ProcessSummary {
            name: p.name.clone(),
            memory_bytes: p.memory_bytes,
        })
    }

    /// 启动两个采集循环。前端未 listen 时 emit 失败会被静默丢弃。
    /// `history` / `alert_history` 为 `None` 时（应用数据目录不可用）只关掉对应那份落盘，
    /// 实时链路不受影响。
    pub fn spawn(
        &self,
        app: AppHandle,
        history: Option<crate::history::HistoryStore>,
        alert_history: Option<crate::history::AlertStore>,
        alerts: Arc<RwLock<crate::alert::AlertConfig>>,
    ) {
        let config = self.config.clone();
        let latest = self.latest.clone();
        {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                run_metrics_loop(app, config, latest, history, alert_history, alerts).await;
            });
        }

        let config = self.config.clone();
        let stream = self.process_stream.clone();
        let latest_processes = self.latest_processes.clone();
        let process_query = self.process_query.clone();
        let process_query_gen = self.process_query_gen.clone();
        let process_page_gen = self.process_page_gen.clone();
        tauri::async_runtime::spawn(async move {
            run_process_loop(
                app,
                config,
                stream,
                latest_processes,
                process_query,
                process_query_gen,
                process_page_gen,
            )
            .await;
        });
    }
}

impl Default for MonitorService {
    fn default() -> Self {
        Self::new()
    }
}

async fn run_metrics_loop(
    app: AppHandle,
    config: Arc<RwLock<MonitorConfig>>,
    latest: Arc<RwLock<Option<MetricsSnapshot>>>,
    history: Option<crate::history::HistoryStore>,
    alert_history: Option<crate::history::AlertStore>,
    alert_config: Arc<RwLock<crate::alert::AlertConfig>>,
) {
    let mut collector = Collector::new();
    let mut recorder = history.map(crate::history::HistoryRecorder::open);
    let mut alert_recorder = alert_history.map(crate::history::AlertRecorder::open);
    let mut alerts = crate::alert::AlertEngine::new();
    // 首个采样点只建立基线，不推送（CPU 增量需要前一次采样）。
    collector.refresh_cpu();
    tokio::time::sleep(Duration::from_millis(300)).await;

    loop {
        let cfg = config.read().map(|g| g.clone()).unwrap_or_default();
        if cfg.paused {
            // 暂停期间没有帧，"连续 N 帧"的计数不能跨过这段空白继续攒。
            alerts.reset();
            tokio::time::sleep(Duration::from_millis(500)).await;
            continue;
        }

        let tick = Duration::from_millis(cfg.interval_ms);
        let frame_started = Instant::now();
        let frame = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            collector.snapshot(Duration::from_millis(cfg.disk_interval_ms))
        }));

        match frame {
            Ok(snapshot) => {
                store(&latest, snapshot.clone());
                if let Err(e) = app.emit(METRICS_EVENT, &snapshot) {
                    static EMIT_WARNED: std::sync::Once = std::sync::Once::new();
                    EMIT_WARNED.call_once(|| {
                        eprintln!("[monitor] 事件 {} 推送失败: {}", METRICS_EVENT, e)
                    });
                }
                #[cfg(debug_assertions)]
                {
                    static FIRST_FRAME_LOGGED: std::sync::Once = std::sync::Once::new();
                    FIRST_FRAME_LOGGED.call_once(|| {
                        eprintln!(
                            "[monitor] {} 首帧: cpu={:.1}% cores={} 内存={}/{} 分区={} 网卡={}",
                            METRICS_EVENT,
                            snapshot.cpu.total,
                            snapshot.cpu.core_count,
                            snapshot.memory.used_bytes,
                            snapshot.memory.total_bytes,
                            snapshot.disks.len(),
                            snapshot.networks.len(),
                        )
                    });
                }

                // 历史落盘（T3-07）：间隔节流在 recorder 内部，这里是每帧都调用。
                if let Some(recorder) = recorder.as_mut() {
                    recorder.observe(crate::history::HistoryPoint::from_snapshot(&snapshot));
                }

                // 告警判定（T5-01）：与历史落盘同一时间轴，用帧自带的时间戳做冷却计算。
                let alert_cfg = alert_config.read().map(|g| g.clone()).unwrap_or_default();
                for event in alerts.evaluate(&snapshot, &alert_cfg) {
                    #[cfg(debug_assertions)]
                    eprintln!(
                        "[alert] {:?} {:?} 值={:.2}% 阈值={:.2}% 连续={} 帧 目标={:?}",
                        event.metric,
                        event.level,
                        event.value,
                        event.threshold,
                        event.consecutive,
                        event.target
                    );
                    // 与 metrics 帧同口径：无人 listen 时 emit 失败就丢弃，不影响采集链路。
                    if let Err(e) = app.emit(crate::alert::ALERT_EVENT, &event) {
                        static ALERT_EMIT_WARNED: std::sync::Once = std::sync::Once::new();
                        ALERT_EMIT_WARNED.call_once(|| {
                            eprintln!("[alert] 事件推送失败: {}", e)
                        });
                    }
                    // 落盘（T5-03）与推送是两件事：界面没打开、事件没人接，历史照样要留下。
                    if let Some(alert_recorder) = alert_recorder.as_mut() {
                        alert_recorder.record(&event);
                    }
                }
            }
            Err(_) => {
                // 采集器 panic 后重建，保证循环不退出。
                collector = Collector::new();
                // 这一帧没采到，连续计数同样要断开。
                alerts.reset();
            }
        }

        // 采集耗时计入间隔；本帧超时则直接进入下一帧，不追赶欠下的 tick。
        let spent = frame_started.elapsed();
        let sleep_for = tick.saturating_sub(spent);
        if !sleep_for.is_zero() {
            tokio::time::sleep(sleep_for).await;
        }
    }
}

async fn run_process_loop(
    app: AppHandle,
    config: Arc<RwLock<MonitorConfig>>,
    stream: Arc<AtomicBool>,
    latest_processes: Arc<RwLock<Option<ProcessPage>>>,
    process_query: Arc<RwLock<ProcessQuery>>,
    process_query_gen: Arc<AtomicUsize>,
    process_page_gen: Arc<AtomicUsize>,
) {
    let mut sys = process_system();
    let mut warmed = false;

    loop {
        if !stream.load(Ordering::Relaxed) {
            tokio::time::sleep(Duration::from_millis(300)).await;
            continue;
        }

        let interval = config
            .read()
            .map(|g| g.process_interval_ms)
            .unwrap_or(3000);

        // 刚开启时先用一个短间隔暖机，让 CPU 列立刻可信。
        if !warmed {
            sys.refresh_processes_specifics(
                ProcessesToUpdate::All,
                true,
                process_refresh_kind(),
            );
            tokio::time::sleep(Duration::from_millis(350)).await;
            warmed = true;
        }

        // 先取代数再读查询：两者之间发生的变更才不会被漏掉。
        let seen_gen = process_query_gen.load(Ordering::Relaxed);
        let query = process_query
            .read()
            .map(|g| g.clone())
            .unwrap_or_default();

        let frame = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            collect_processes(&mut sys, warmed, &query)
        }));

        match frame {
            Ok(page) => {
                store(&latest_processes, page.clone());
                // 帧入库后才推进页号，避免读侧看到"号已新、页还旧"。
                process_page_gen.store(seen_gen, Ordering::Relaxed);
                let _ = app.emit(PROCESSES_EVENT, &page);
                #[cfg(debug_assertions)]
                {
                    static FIRST_FRAME_LOGGED: std::sync::Once = std::sync::Once::new();
                    FIRST_FRAME_LOGGED.call_once(|| {
                        let head = page
                            .items
                            .first()
                            .map(|p| format!("{}(pid={})", p.name, p.pid))
                            .unwrap_or_else(|| "无".to_string());
                        eprintln!(
                            "[monitor] {} 首帧: 命中 {} 进程 / 返回 {} 行 / 排序={:?} / 关键字={:?} / warming={} / 首位={}",
                            PROCESSES_EVENT,
                            page.total,
                            page.items.len(),
                            query.sort_by,
                            query.keyword,
                            page.warming,
                            head,
                        )
                    });
                }
            }
            Err(_) => {
                // 重建必须走 `process_system()`：`System::new()` 会把 CPU 基准一起丢掉，
                // 于是此后第一帧又是全 0（H-06 的同一条坑）。
                sys = process_system();
                warmed = false;
            }
        }

        // 等满周期，但查询一变就提前出帧，搜索/排序不必等下一个 3s。
        let deadline = Instant::now() + Duration::from_millis(interval);
        while Instant::now() < deadline {
            if process_query_gen.load(Ordering::Relaxed) != seen_gen {
                break;
            }
            tokio::time::sleep(Duration::from_millis(80)).await;
        }
    }
}

// ==================== 工具 ====================

fn store<T>(slot: &Arc<RwLock<Option<T>>>, value: T) {
    if let Ok(mut guard) = slot.write() {
        *guard = Some(value);
    }
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

pub fn pressure_for(usage_percent: f64) -> MemoryPressure {
    if usage_percent >= 90.0 {
        MemoryPressure::Critical
    } else if usage_percent >= 75.0 {
        MemoryPressure::Warning
    } else {
        MemoryPressure::Normal
    }
}

pub fn epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

pub fn epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_clamps_to_safe_ranges() {
        let c = MonitorConfig {
            interval_ms: 1,
            process_interval_ms: 1,
            disk_interval_ms: u64::MAX,
            paused: false,
        }
        .clamped();
        assert_eq!(c.interval_ms, MIN_INTERVAL_MS);
        assert_eq!(c.process_interval_ms, 1_000);
        assert_eq!(c.disk_interval_ms, 300_000);
    }

    #[test]
    fn pressure_thresholds() {
        assert_eq!(pressure_for(10.0), MemoryPressure::Normal);
        assert_eq!(pressure_for(80.0), MemoryPressure::Warning);
        assert_eq!(pressure_for(95.0), MemoryPressure::Critical);
    }

    #[test]
    fn collector_produces_non_zero_core_count_and_totals() {
        let mut collector = Collector::new();
        collector.refresh_cpu();
        std::thread::sleep(Duration::from_millis(250));
        collector.refresh_cpu();

        let cpu = collector.cpu_metrics();
        assert!(cpu.core_count > 0);
        assert_eq!(cpu.per_core.len(), cpu.core_count);
        assert!(cpu.total < 100.001, "cpu total must stay a percentage");

        let memory = collector.memory_metrics();
        assert!(memory.total_bytes > 0);
        assert!(memory.usage_percent > 0.0);
        assert!(memory.available_bytes > 0);
        assert!(
            memory.used_bytes + memory.available_bytes <= memory.total_bytes * 101 / 100,
            "used+available 不应显著超过 total: {memory:?}"
        );
    }

    #[test]
    fn network_metrics_carry_real_addresses() {
        let mut collector = Collector::new();
        let nets = collector.network_metrics();
        assert!(!nets.is_empty());
        for n in &nets {
            // 回归 P0-5：绝不允许再出现占位 MAC 或 Debug 字符串当 IP。
            if let Some(mac) = &n.mac {
                assert_ne!(mac, "AA:BB:CC:DD:EE:FF");
                assert_eq!(mac.matches(':').count(), 5);
            }
            if let Some(ip) = &n.ipv4 {
                assert!(ip.parse::<std::net::Ipv4Addr>().is_ok(), "bad ipv4 {:?}", ip);
            }
            if let Some(ip) = &n.ipv6 {
                assert!(ip.parse::<std::net::Ipv6Addr>().is_ok(), "bad ipv6 {:?}", ip);
            }
        }
    }

    #[test]
    fn disk_metrics_have_no_io_before_the_first_refresh_window() {
        // 窗口未到 = 没有任何增量可说，必须给 None，不能拿 0 冒充"空闲"。
        let mut collector = Collector::new();
        let mut seen_total = false;
        for _ in 0..2 {
            let disks = collector.disk_metrics(Duration::from_secs(300));
            assert!(!disks.is_empty(), "至少应枚举到一个磁盘");
            for d in &disks {
                seen_total |= d.total_bytes > 0;
                assert_eq!(d.read_bytes_per_sec, None, "未刷新窗口不得给出读速 {:?}", d);
                assert_eq!(d.write_bytes_per_sec, None, "未刷新窗口不得给出写速 {:?}", d);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(seen_total);
    }

    #[test]
    fn disk_io_rates_come_from_a_real_refresh_window() {
        let mut collector = Collector::new();
        std::thread::sleep(Duration::from_millis(220));
        let disks = collector.disk_metrics(Duration::from_millis(200));
        let mut known = 0usize;
        for d in &disks {
            match (d.read_bytes_per_sec, d.write_bytes_per_sec) {
                (Some(r), Some(w)) => {
                    known += 1;
                    assert!(r.is_finite() && r >= 0.0, "读速率失真 {:?}", d);
                    assert!(w.is_finite() && w >= 0.0, "写速率失真 {:?}", d);
                }
                (None, None) => {}
                mixed => panic!("读写速率必须同时有值或同时为空：{:?}", mixed),
            }
        }
        // macOS 走 IOKit 块设备计数，内部盘必然有来源；其它平台允许全为 None（如拿不到计数）。
        #[cfg(target_os = "macos")]
        assert!(known > 0, "macOS 上应至少有一个挂载点报出真实 I/O 计数");
        // 非 macOS 上这个计数没有可断言的下界（拿不到 I/O 计数是合法结果），显式丢弃，
        // 免得 Windows/Linux 腿报 "assigned to, but never used" 把真 warning 淹掉。
        #[cfg(not(target_os = "macos"))]
        let _ = known;
    }

    #[test]
    fn disk_io_rate_is_window_average_and_does_not_decay_between_frames() {
        let mut collector = Collector::new();
        std::thread::sleep(Duration::from_millis(220));
        let first = collector.disk_metrics(Duration::from_millis(200));
        // 同一窗口内再取两帧：速率必须原样沿用，不能因分母继续长大而越显示越小。
        let second = collector.disk_metrics(Duration::from_millis(200));
        let third = collector.disk_metrics(Duration::from_millis(200));
        let pick = |frames: &[DiskMetrics]| {
            frames
                .iter()
                .map(|d| (d.mount_point.clone(), d.read_bytes_per_sec, d.write_bytes_per_sec))
                .collect::<Vec<_>>()
        };
        assert_eq!(pick(&first), pick(&second));
        assert_eq!(pick(&second), pick(&third));
    }

    #[test]
    fn process_collection_is_sorted_and_truncated() {
        let mut sys = process_system();
        // 第一条是"填基准"的采样，第二条才是取差值的那条（H-06）。
        collect_processes(&mut sys, false, &ProcessQuery::default());
        let page = collect_processes(&mut sys, true, &ProcessQuery::default());
        assert!(page.total > 5);
        assert!(!page.warming);
        // 默认一页 50 行：不再是"全量返回后截断"
        assert!(page.items.len() <= DEFAULT_PROCESS_PAGE_SIZE);
        assert!(page.total >= page.items.len());
        for w in page.items.windows(2) {
            assert!(w[0].cpu_usage >= w[1].cpu_usage, "not sorted by cpu");
        }
        assert!(page.items.iter().all(|p| !p.name.is_empty()));

        // 越界 offset 只给空页，不能 panic。
        let beyond = collect_processes(
            &mut sys,
            true,
            &ProcessQuery {
                offset: page.total + 1000,
                ..Default::default()
            },
        );
        assert!(beyond.items.is_empty());
    }

    /// H-06 回归：进程 CPU 必须采到**真实差值**。
    /// 冷 `System` 的 CPU 要"建表 → 填基准 → 取差值"三步，少一步就整列 0；
    /// 而上面那条排序断言 `>=` 在全 0 上照样成立，所以旧测试一个都抓不到 —— 这条用本机负载把它钉住。
    #[test]
    fn per_process_cpu_is_actually_sampled() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let stop = Arc::new(AtomicBool::new(false));
        let burn = {
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                let mut acc: u64 = 0;
                while !stop.load(Ordering::Relaxed) {
                    acc = acc.wrapping_mul(31).wrapping_add(7);
                    std::hint::black_box(acc);
                }
            })
        };

        // 关键字是纯数字时 `matches` 按 PID 精确命中，结果里只有自己这一行。
        let me = std::process::id();
        let page = collect_processes_warmed(&ProcessQuery {
            keyword: me.to_string(),
            sort_by: ProcessSort::Cpu,
            desc: true,
            ..Default::default()
        });
        stop.store(true, Ordering::Relaxed);
        let _ = burn.join();

        let mine = page
            .items
            .iter()
            .find(|p| p.pid == me)
            .unwrap_or_else(|| panic!("按 PID 检索没命中自己 {me}：{:?}", page.items));
        assert!(
            mine.cpu_usage > 0.0,
            "进程 CPU 没采到真实差值（实际 {}）：三步暖机被改坏了",
            mine.cpu_usage
        );
        assert!(mine.memory_bytes > 0, "内存必须同时有来源：{mine:?}");
    }

    fn row(pid: u32, name: &str, cpu_usage: f64, memory_bytes: u64) -> ProcessInfo {
        ProcessInfo {
            pid,
            name: name.to_string(),
            cpu_usage,
            memory_bytes,
            threads: None,
            user_name: None,
            parent_pid: None,
            run_time_seconds: None,
        }
    }

    /// 过滤/排序/夹紧都是纯逻辑，用固定行集断言，避免依赖本机进程抖动。
    #[test]
    fn process_query_filters_sorts_and_normalizes() {
        let rows = vec![
            row(10, "Finder", 1.0, 500),
            row(20, "Google Chrome", 9.0, 100),
            row(30, "terminal", 5.0, 900),
        ];

        let chrome = ProcessQuery {
            keyword: "chrome".to_string(),
            ..Default::default()
        };
        assert!(chrome.matches(&rows[1]));
        assert!(!chrome.matches(&rows[0]));
        // 关键字命中 PID 同样有效
        assert!(ProcessQuery {
            keyword: "30".to_string(),
            ..Default::default()
        }
        .matches(&rows[2]));

        let by_memory = ProcessQuery {
            sort_by: ProcessSort::Memory,
            desc: true,
            ..Default::default()
        };
        let mut sorted = rows.clone();
        sorted.sort_by(|a, b| by_memory.compare(a, b));
        assert_eq!(
            sorted.iter().map(|p| p.pid).collect::<Vec<_>>(),
            vec![30, 10, 20]
        );

        let by_pid_asc = ProcessQuery {
            sort_by: ProcessSort::Pid,
            desc: false,
            ..Default::default()
        };
        let mut asc = rows.clone();
        asc.sort_by(|a, b| by_pid_asc.compare(a, b));
        assert_eq!(
            asc.iter().map(|p| p.pid).collect::<Vec<_>>(),
            vec![10, 20, 30]
        );

        let q = ProcessQuery {
            keyword: "  CHROME ".to_string(),
            limit: 100_000,
            ..Default::default()
        }
        .normalized();
        assert_eq!(q.keyword, "chrome");
        assert_eq!(q.limit, MAX_PROCESSES_PER_PAGE);
        assert_eq!(
            ProcessQuery {
                limit: 0,
                ..Default::default()
            }
            .normalized()
            .limit,
            1
        );
    }

    #[test]
    fn looks_up_own_process_by_pid() {
        let me = std::process::id();
        let found = lookup_process(me).expect("self pid must resolve");
        assert!(!found.name.is_empty());
        assert!(found.memory_bytes > 0);
        assert!(lookup_process(u32::MAX - 1).is_none());
    }

    #[test]
    fn process_detail_exposes_paths_but_never_command_line_or_env() {
        let me = std::process::id();
        let detail = collect_process_detail(me).expect("self pid must resolve");
        assert_eq!(detail.pid, me);
        assert!(detail.is_self);
        assert!(!detail.name.is_empty());
        assert!(detail.memory_bytes > 0);
        assert!(detail.start_time > 1_700_000_000);
        // 测试进程刚被 cargo 拉起，run_time 因秒级精度可能为 0，只要求它不离谱。
        assert!(detail.run_time_seconds < 3_600);
        assert!(detail.parent_pid.is_some(), "测试进程必然有父进程");
        assert!(
            detail.exe_path.as_deref().unwrap_or("").contains("target"),
            "自身 exe 路径应可读取，实际 {:?}",
            detail.exe_path
        );
        assert!(!detail.status.is_empty());

        // 04 数据安全：详情面板只给路径与时间，命令行参数与环境变量一律不出后端。
        let keys: Vec<String> = serde_json::to_value(&detail)
            .expect("serializable")
            .as_object()
            .expect("object")
            .keys()
            .cloned()
            .collect();
        assert!(
            !keys.iter().any(|k| k.contains("cmd") || k.contains("env")),
            "详情字段泄漏命令行/环境变量: {keys:?}"
        );

        assert!(collect_process_detail(u32::MAX - 1).is_none());
    }

    #[test]
    fn process_cache_is_treated_as_stale_right_after_a_query_change() {
        let service = MonitorService::new();
        assert!(service.processes_match_query());

        // 与当前生效值相同的查询不推进代数，缓存继续可用（前端每次挂载都会重发一次查询）。
        service.set_process_query(ProcessQuery::default());
        assert!(service.processes_match_query());

        let applied = service.set_process_query(ProcessQuery {
            keyword: " Chrome ".into(),
            ..Default::default()
        });
        assert_eq!(applied.keyword, "chrome");
        assert!(
            !service.processes_match_query(),
            "查询变更后、采集线程产出新帧之前，缓存必须判为过期，否则 get_processes 会返回上一次的关键字"
        );
    }

    /// 取出 `export const NAME = <数字>;` 里的数字（`name` 需自带结尾的 ` =`）。
    fn ts_number(name: &str, src: &str) -> usize {
        src.split(name)
            .nth(1)
            .unwrap_or_default()
            .trim_start()
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse::<usize>()
            .unwrap_or_else(|_| panic!("前端 {name} 不是数字"))
    }

    /// T5-10 把页容量交给用户选，选项表在前端 `lib/ui.ts`、上限在后端。
    /// 后端会静默夹取越界的 `limit`，于是"前端显示 500 行/页、实际按 300 取"这种错位
    /// 只能靠这条契约锁住 —— 和前端的 prefs 契约测试同一思路：不改代码就必须在测试里对齐。
    #[test]
    fn frontend_page_sizes_stay_inside_the_backend_cap() {
        let ui_ts = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/lib/ui.ts"),
        )
        .expect("前端 lib/ui.ts 必须存在");
        let list = ui_ts
            .split("PROCESS_PAGE_SIZE_OPTIONS")
            .nth(1)
            .and_then(|rest| rest.split('[').nth(1))
            .and_then(|inner| inner.split(']').next())
            .expect("PROCESS_PAGE_SIZE_OPTIONS 必须是数字数组");
        let options: Vec<usize> = list
            .split(',')
            .map(|s| s.trim().parse::<usize>().expect("页容量选项必须是整数"))
            .collect();
        assert!(!options.is_empty(), "页容量选项不能为空");
        for size in &options {
            assert!(
                *size >= 1 && *size <= MAX_PROCESSES_PER_PAGE,
                "页容量 {size} 越过后端上限 {MAX_PROCESSES_PER_PAGE}，会被 normalized() 静默夹取"
            );
        }

        // 名称以 `= ` 收尾锁定声明处：PROCESS_PAGE_SIZE 是 PROCESS_PAGE_SIZE_OPTIONS 的前缀，
        // 直接按名字切会先撞上选项表那一行。
        let default_page_size = ts_number("PROCESS_PAGE_SIZE =", &ui_ts);
        assert!(
            options.contains(&default_page_size),
            "默认页容量 {default_page_size} 不在选项表 {options:?} 里"
        );

        let virtual_threshold = ts_number("PROCESS_VIRTUAL_THRESHOLD =", &ui_ts);
        let largest_option = *options.iter().max().unwrap();
        assert!(
            virtual_threshold >= default_page_size && virtual_threshold < largest_option,
            "虚拟滚动阈值 {virtual_threshold} 必须让默认 {default_page_size} 行整页渲染、\
             又让最大页容量 {largest_option} 行走虚拟窗口"
        );
    }

    /// T5-07 的迷你模式只允许是"已有帧的另一种排布"。这条审计钉住三件事：
    /// ① 组件自己不开任何通路 —— 多一条采集链路就会让同一个指标出现两套节奏，而"再给它加个
    ///    专用接口"正是这类紧凑视图最自然的膨胀方向；
    /// ② 不落 localStorage —— 迷你开关刻意只是会话级，不进 T5-11 那份 5 项快照（快照字段
    ///    由 Rust/TS 契约测试对齐，加第 6 项等于改偏好文件的语义）；
    /// ③ "哪块盘"与"趋势窗口"复用同一份函数，不在这里复刻第二套口径 —— 后端只按一份规则
    ///    挑最满的分区，界面就不能自己再算一次，否则会出现"告警报的盘"和"迷你条上的盘"不是一块。
    #[test]
    fn mini_menubar_is_a_pure_projection_of_existing_streams() {
        let mini =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/components/MiniMenuBar.tsx");
        let src = std::fs::read_to_string(&mini).expect("MiniMenuBar.tsx 必须存在");
        for forbidden in [
            "invoke(",
            "@tauri-apps/api",
            "Commands.",
            "fetch(",
            "new EventSource",
            "localStorage",
            "d.totalBytes > 0",
            "getCurrentWindow",
            "setSize",
        ] {
            assert!(
                !src.contains(forbidden),
                "迷你模式里出现了不该有的通路 {forbidden}（{}）",
                mini.display()
            );
        }
        for required in [
            "busiestDisk(",
            "windowHistory(",
            "downsampleHistory(",
            "onExit",
        ] {
            assert!(
                src.contains(required),
                "迷你模式应复用同一份口径或出口 {required}"
            );
        }
    }

    /// 迷你模式与主界面共用 `App` 里那份状态。这两个接线断掉都不会报错，只会静默变慢或误触：
    /// 进程流不在迷你模式下停下，就等于看不见的表还在 3 秒枚举一次全机器进程；
    /// 快捷键不避开输入焦点，进程表搜索框里打一个 `m` 就会把界面切走。
    #[test]
    fn mini_mode_is_wired_into_the_process_stream_and_the_key_guard() {
        let app = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/App.tsx"),
        )
        .expect("src/App.tsx 必须存在");
        assert!(
            app.contains("activeTab === \"processes\" && !miniMode"),
            "进入迷你模式时要让进程流停下（useProcessStream 的 active 参数），否则看不见的表仍在 3 秒枚举一次"
        );
        assert!(
            app.contains("isContentEditable"),
            "键盘切换迷你模式要跳过正在输入的目标"
        );
        assert!(
            app.contains("<ErrorBoundary label=\"迷你模式\">"),
            "迷你模式要有自己的错误边界，不能让它抛错时整个界面塌掉"
        );
    }
}
