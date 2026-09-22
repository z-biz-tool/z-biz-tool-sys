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
            let key = disk.mount_point().to_string_lossy().into_owned();
            // 首个刷新窗口到来之前没有任何增量可说，保持 None。
            let rates = self.disk_rates.get(&key).copied().unwrap_or((None, None));

            out.push(disk_entry(
                disk.name().to_string_lossy().into_owned(),
                key,
                disk.file_system().to_string_lossy().into_owned(),
                disk.total_space(),
                disk.available_space(),
                rates,
            ));
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
        notify: crate::notify::NotifyState,
    ) {
        let config = self.config.clone();
        let latest = self.latest.clone();
        {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                run_metrics_loop(app, config, latest, history, alert_history, alerts, notify).await;
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
    notify: crate::notify::NotifyState,
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
                    // 系统通知（T5-02）：判定已经由引擎做完，这里只是把同一条再投一次。
                    // 投递结果绝不能影响上面两件事，所以 `post_alert` 内部吞掉错误、只记账。
                    crate::notify::post_alert(&app, &notify, &event);
                }
            }
            Err(payload) => {
                // FI-03：恢复动作收在 `recover_metrics_after_panic` 里（日志 + 重建 + 断计数），
                // 这样"恢复"本身能被单测真跑，而不是在测试里重抄一遍循环体的两行。
                recover_metrics_after_panic(&mut collector, &mut alerts, panic_reason(payload));
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
            Err(payload) => {
                // FI-03（进程侧）：同一套恢复口径 —— 留日志、走 `process_system()` 重建、回到暖机。
                recover_processes_after_panic(&mut sys, &mut warmed, panic_reason(payload));
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

/// 一个分区的 `(总容量, 可用量)` 变成界面要的那几个字段。
///
/// 抽成纯函数是为了 FI-04：真机上"拿不到容量的分区"不是每次都能造出来（本机 `分区=2` 两块都有容量），
/// 而"容量拿不到 ⇒ 只把这一块标成不可用、既不污染别的块、也不给出一个看起来正常的 0 %"这条规则
/// 必须能被直接注入测到。`usage_percent` 的类型是 `f64`（不是 Option），所以拿不到容量时它仍是 `0.0`，
/// 但同一条记录带着 `available: false` —— 读到 `false` 必须显示 `—`，这条在 `DiskTab` 那侧有回归盯着。
fn disk_entry(
    name: String,
    mount_point: String,
    file_system: String,
    total: u64,
    available: u64,
    rates: (Option<f64>, Option<f64>),
) -> DiskMetrics {
    let usable = total > 0;
    let used = total.saturating_sub(available);
    DiskMetrics {
        name,
        mount_point,
        file_system,
        available: usable,
        total_bytes: total,
        used_bytes: used,
        available_bytes: available,
        usage_percent: if usable {
            round2(used as f64 / total as f64 * 100.0)
        } else {
            0.0
        },
        read_bytes_per_sec: rates.0,
        write_bytes_per_sec: rates.1,
    }
}

/// 把 `catch_unwind` 的载荷变成可读的一句话，并与落盘/日志同口径过一遍脱敏
/// （采集线程的 panic 文本会打到用户的终端上，不许带出家目录或内网地址）。
/// 抓不到可读字符串就写"未知"，不拿 `{:?}` 把一个空串当成原因报出去
/// —— `Box<dyn Any>` 的 `Debug` 只会打出 `Any { .. }`，走 `{:?}` 等于永远"未知"。
///
/// 参数**必须按值收 `Box`**：写成 `&(dyn Any + Send)` 再传 `&payload`（payload 是 `Box<...>`）
/// 会让 downcast 看到的是那个盒子而不是盒子里的东西，于是一条正常的 `panic!("…")`
/// 也会被打成"未知 panic"。这个错是 `fi03_probe_what_a_panic_payload_actually_carries`
/// 在本轮实测里抓到的。
fn panic_reason(payload: Box<dyn std::any::Any + Send>) -> String {
    for candidate in [
        payload.downcast_ref::<String>().map(String::as_str),
        payload.downcast_ref::<&str>().copied(),
    ]
    .into_iter()
    .flatten()
    {
        if !candidate.trim().is_empty() {
            return crate::log_sanitize::sanitize(candidate);
        }
    }
    "未知 panic（载荷里没有可读的原因）".to_string()
}

/// FI-03：metrics 帧 panic 后的恢复动作，收成一个函数，好让"恢复"这件事本身可测。
///
/// 三件事缺一不可，而且顺序不能反：
/// 1. **留日志** —— 静默重建等于"界面偶尔停一下又自己好了"，谁都没法查；这条不是 debug 专属，
///    panic 是低频且必须被看见的事件。
/// 2. **重建采集器** —— `sysinfo` 内部存的是"上次采样值 + 时间戳"的差值基准，panic 可能把它留在
///    半更新状态，继续用会算出假值（H-06 同一类坑）。
/// 3. **断开告警的连续计数** —— 这一帧没采到，把前后两帧当成"连续超限"就会报出没发生过的告警。
fn recover_metrics_after_panic(
    collector: &mut Collector,
    alerts: &mut crate::alert::AlertEngine,
    reason: String,
) {
    eprintln!("[monitor] 采集帧 panic，已重建采集器并断开告警的连续计数：{reason}");
    *collector = Collector::new();
    alerts.reset();
}

/// FI-03（进程循环那一侧）。重建必须走 `process_system()`：`System::new()` 会把 CPU 基准一起丢掉，
/// 于是此后第一帧又是全 0（H-06）；`warmed = false` 则让那一帧**明确标成暖机中**，
/// 而不是给一排 0 % 的排行榜。
fn recover_processes_after_panic(sys: &mut System, warmed: &mut bool, reason: String) {
    eprintln!("[monitor] 进程帧 panic，已重建进程表并回到暖机：{reason}");
    *sys = process_system();
    *warmed = false;
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

    // ==================== FI-01~FI-04 故障注入（06 文档"尚未自动化"那一项）====================

    /// CPU 99 % 的一帧：越过默认的 critical 95 %，用来驱动"连续 N 帧"这条链。
    fn hot_frame(timestamp_ms: u64) -> MetricsSnapshot {
        MetricsSnapshot {
            timestamp_ms,
            uptime_seconds: 100,
            cpu: CpuMetrics {
                total: 99.0,
                per_core: vec![99.0],
                core_count: 1,
            },
            memory: MemoryMetrics {
                total_bytes: 16 * 1024 * 1024 * 1024,
                used_bytes: 1024 * 1024 * 1024,
                available_bytes: 15 * 1024 * 1024 * 1024,
                swap_total_bytes: 0,
                swap_used_bytes: 0,
                usage_percent: 6.25,
                pressure: MemoryPressure::Normal,
            },
            disks: vec![disk_entry(
                "Data".to_string(),
                "/System/Volumes/Data".to_string(),
                "apfs".to_string(),
                100,
                50,
                (None, None),
            )],
            networks: Vec::new(),
        }
    }

    /// 先量清楚 `catch_unwind` 的载荷里到底能拿到什么，再决定 `panic_reason` 怎么写。
    /// 这条是"临时探针转正"：本仓库踩过的教训是 —— 不看真实形状就写提取逻辑，
    /// 结果是日志一路打"未知 panic"，看着像没有原因，其实是取原因的方法错了。
    #[test]
    fn fi03_probe_what_a_panic_payload_actually_carries() {
        let forms = [
            std::panic::catch_unwind(|| panic!("注入：字面量")),
            std::panic::catch_unwind(|| panic!("注入：{}", 42)),
            // 注意：`panic!(42u32)` 在这个 edition 里根本编不过（"format argument must be a string
            // literal"），非字符串载荷只能由 `panic_any` 产生 —— 这也是一种载荷形状。
            std::panic::catch_unwind(|| std::panic::panic_any(42u32)),
        ];
        let mut readable = 0;
        let mut shapes = Vec::new();
        for payload in forms.into_iter().filter_map(|r| r.err()) {
            let as_string = payload.downcast_ref::<String>().cloned();
            let as_str = payload.downcast_ref::<&str>().copied().map(String::from);
            let reason = panic_reason(payload);
            shapes.push(format!(
                "String={as_string:?} &str={as_str:?} -> {reason}"
            ));
            if !reason.starts_with("未知") {
                readable += 1;
            }
        }
        // 断言的是"至少字面量这一种必须能读出原因"，读不出的那种（`panic!(42u32)`）就承认读不出。
        assert!(readable >= 1, "一种原因都读不出来：{shapes:?}");
        for shape in &shapes {
            println!("FI-03 载荷形状: {shape}");
        }
    }

    /// FI-03：panic 之后**必须断掉"连续 N 帧"这条链**。
    /// 少了这一步，恢复后的第一帧会被当成第 3 帧，于是弹出一条这台机器从没连续超限过的告警。
    #[test]
    fn fi03_a_panicked_frame_breaks_the_alert_chain() {
        let mut collector = Collector::new();
        let mut engine = crate::alert::AlertEngine::new();
        let cfg = crate::alert::AlertConfig::default();
        assert_eq!(cfg.consecutive, 3, "这条测试的前提是默认连续 3 帧");

        // 先攒两帧越限：还差一帧就该触发。
        assert!(engine.evaluate(&hot_frame(1_000), &cfg).is_empty());
        assert!(engine.evaluate(&hot_frame(2_000), &cfg).is_empty());

        // 走真实的恢复入口（循环里调的就是它），不是重抄一遍。
        let boom = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| panic!("注入：采集器 panic")));
        recover_metrics_after_panic(
            &mut collector,
            &mut engine,
            panic_reason(boom.expect_err("注入的 panic 必须被捕获")),
        );

        // 计数已断开：这帧不该触发。
        assert!(
            engine.evaluate(&hot_frame(3_000), &cfg).is_empty(),
            "panic 后第一帧就被当成'连续第 3 帧'，会报出没发生过的告警"
        );
        // 再两帧之后才触发 —— 引擎在恢复后仍然可用。
        assert!(engine.evaluate(&hot_frame(4_000), &cfg).is_empty());
        let fired = engine.evaluate(&hot_frame(5_000), &cfg);
        assert_eq!(fired.len(), 1, "恢复后本该重新攒满 3 帧再触发：{fired:?}");
        assert_eq!(fired[0].metric, crate::alert::AlertMetric::Cpu);
    }

    /// FI-03：重建出来的采集器必须还能交出可用的一帧（不是"重建了个空壳"）。
    #[test]
    fn fi03_the_rebuilt_collector_still_produces_a_usable_frame() {
        let mut collector = Collector::new();
        let mut engine = crate::alert::AlertEngine::new();
        let _ = collector.snapshot(Duration::from_millis(0));

        let boom = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| panic!("注入：重建用的采集器")));
        recover_metrics_after_panic(
            &mut collector,
            &mut engine,
            panic_reason(boom.expect_err("注入的 panic 必须被捕获")),
        );

        // 重建后的第一帧：结构上必须完整（CPU 百分比在 0..=100、核心数与内存容量非零）。
        // 磁盘刷新间隔用生产同款：`Collector::new()` 刚把 `last_disk_sample` 设成 now，
        // 所以这一帧没有窗口可算速率 —— 必须是 `None`，写 0 就是造数。
        let frame = collector.snapshot(Duration::from_secs(5));
        assert!(frame.cpu.total >= 0.0 && frame.cpu.total <= 100.0, "{:?}", frame.cpu.total);
        assert!(frame.cpu.core_count > 0);
        assert!(frame.memory.total_bytes > 0, "重建后拿不到内存容量就是空壳");
        assert!(!frame.networks.is_empty(), "重建后网卡列表为空");
        // 首帧的速率列必须是 None（还没有窗口可算），不能是 0。
        for disk in &frame.disks {
            assert!(
                disk.read_bytes_per_sec.is_none() && disk.write_bytes_per_sec.is_none(),
                "刚重建就报出速率 = 造数：{:?}",
                (disk.mount_point.clone(), disk.read_bytes_per_sec, disk.write_bytes_per_sec)
            );
        }
    }

    /// FI-03（进程那一侧）：恢复后回到暖机中，于是那一帧**明确标注**在暖机，
    /// 而不是给出一排 0 % 的排行（H-06 的原始症状）。
    #[test]
    fn fi03_the_process_recovery_restarts_warming_instead_of_a_zero_leaderboard() {
        let mut sys = process_system();
        let mut warmed = true;
        let query = ProcessQuery::default();

        let boom = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| panic!("注入：进程帧 panic")));
        recover_processes_after_panic(
            &mut sys,
            &mut warmed,
            panic_reason(boom.expect_err("注入的 panic 必须被捕获")),
        );
        assert!(!warmed, "恢复后必须回到暖机");

        let page = collect_processes(&mut sys, warmed, &query);
        assert!(page.warming, "恢复后的第一帧要标明还在暖机，不能让一排 0 % 冒充排行");
        assert!(page.total > 0, "进程表本身就是空的，这条注入没跑到真枚举");
    }

    /// FI-03 的护栏：两处恢复都必须留日志，且循环里不许再留静默的 `Err(_) =>`。
    /// 静默重启的表现是"界面偶尔停一下又自己好了"，谁都无法追查。
    #[test]
    fn fi03_neither_recovery_may_restart_the_loop_silently() {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let production = std::fs::read_to_string(manifest.join("src/monitor.rs"))
            .unwrap()
            .split("#[cfg(test)]")
            .next()
            .expect("monitor.rs 应有测试模块")
            .to_string();

        assert!(
            !production.contains("Err(_) =>"),
            "还有静默的 `Err(_) =>` 分支：恢复动作必须走带日志的 recover_* 函数"
        );
        assert_eq!(
            production.matches("Err(payload) =>").count(),
            2,
            "两个采集循环都该走恢复函数，多一处少一处都要重新看一眼"
        );
        assert!(production.contains("recover_metrics_after_panic(&mut collector, &mut alerts, panic_reason(payload))"));
        assert!(production.contains("recover_processes_after_panic(&mut sys, &mut warmed, panic_reason(payload))"));

        for name in ["recover_metrics_after_panic", "recover_processes_after_panic"] {
            let needle = format!("fn {name}(");
            let body = production
                .split(needle.as_str())
                .nth(1)
                .unwrap_or_default()
                .split("\n}")
                .next()
                .unwrap_or_default()
                .to_string();
            assert!(body.contains("eprintln!"), "{name} 没留日志：{body}");
        }
        // panic 载荷里的文本要过脱敏（这条日志会打到用户终端上）。
        let reason_fn = production
            .split("fn panic_reason(")
            .nth(1)
            .expect("应有 panic_reason")
            .split("\n}")
            .next()
            .unwrap()
            .to_string();
        assert!(reason_fn.contains("log_sanitize::sanitize"), "panic 原因没脱敏");
    }

    /// FI-04：拿不到容量的分区只标记它自己。
    /// 真机上"网络挂载断开"不是每次都能造出来，所以从 `disk_entry` 这个纯函数直接注入。
    #[test]
    fn fi04_a_partition_without_capacity_is_marked_alone() {
        let dead = disk_entry(
            "afp://10.0.0.8/".to_string(),
            "/Volumes/断了".to_string(),
            "smbfs".to_string(),
            0,
            0,
            (None, None),
        );
        assert!(!dead.available, "容量为 0 的分区必须标成不可用");
        assert_eq!(dead.total_bytes, 0);
        assert_eq!(dead.usage_percent, 0.0, "不可用的分区不该有使用率");
        assert!(
            dead.read_bytes_per_sec.is_none() && dead.write_bytes_per_sec.is_none(),
            "断掉的挂载点不该有速率读数"
        );

        // 同一批里其它分区完全不受影响。
        let siblings = [
            disk_entry("Data".to_string(), "/System/Volumes/Data".to_string(), "apfs".to_string(), 500, 100, (Some(1.0), Some(2.0))),
            disk_entry("全满".to_string(), "/Volumes/Full".to_string(), "apfs".to_string(), 100, 0, (None, None)),
        ];
        assert!(siblings.iter().all(|d| d.available), "有容量的分区不该被邻居带崩");
        assert_eq!(siblings[0].usage_percent, 80.0);
        assert_eq!(siblings[1].usage_percent, 100.0, "可用 0 但容量已知的分区是真的全满");

        // 荒谬组合（容量报 0、可用却 > 0）：仍然只标记这一块不可用，
        // 且 `used` 走 saturating_sub，不许把 u64 减成一个巨大的已用量。
        let absurd = disk_entry(
            "假死".to_string(),
            "/Volumes/Bad".to_string(),
            "nullfs".to_string(),
            0,
            999,
            (None, None),
        );
        assert!(!absurd.available);
        assert_eq!(absurd.used_bytes, 0, "u64 回绕：{}", absurd.used_bytes);
        assert_eq!(absurd.usage_percent, 0.0);

        // 告警侧的口径（同一批数据）：只有可用且有容量的块参与判定。
        let mut engine = crate::alert::AlertEngine::new();
        let cfg = crate::alert::AlertConfig {
            // 磁盘 90/95，连续 1 帧：把三块已知容量的摆在一起，最满的那块该是 100 % 的"全满"
            disk: crate::alert::AlertThresholds {
                warning: 90.0,
                critical: 95.0,
            },
            consecutive: 1,
            ..Default::default()
        };
        let mut frame = hot_frame(10_000);
        frame.cpu.total = 1.0; // 只测磁盘这条路径
        frame.memory.usage_percent = 1.0;
        frame.memory.used_bytes = 1;
        frame.disks = [siblings.to_vec(), vec![dead, absurd]].concat();
        let fired = engine.evaluate(&frame, &cfg);
        let disk_alerts: Vec<_> = fired
            .iter()
            .filter(|e| e.metric == crate::alert::AlertMetric::Disk)
            .collect();
        assert_eq!(disk_alerts.len(), 1, "一块全满 + 一块不可用 + 一块 80 %：只该报一条， got {fired:?}");
        assert_eq!(disk_alerts[0].target.as_deref(), Some("/Volumes/Full"));

        // 显示层的同一口径也要钉住：`available: false` 那一行不许把 `usagePercent` 的 0.0
        // 画成"用了 0 %"的进度条 —— 后端类型上给不出 `None`，这一列就是最后一道闸。
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let tab = std::fs::read_to_string(manifest.join("../src/components/tabs/DiskTab.tsx")).unwrap();
        assert!(tab.contains("const MISSING = \"—\""), "分区表没有统一的缺测画法");
        assert!(tab.contains("disk.available"), "分区表没再看 available 标志");
        let usage_col = tab
            .split("title: \"使用率\"")
            .nth(1)
            .expect("应有使用率那一列")
            .split("},\n          {")
            .next()
            .unwrap_or_default()
            .to_string();
        assert!(
            usage_col.contains("disk.available") && usage_col.contains("MISSING"),
            "使用率那一列还在无条件画进度条，等于把断掉的挂载点显示成 0 %：{usage_col}"
        );
    }

    /// FI-02：没有 listener 时后端不许堆积。共享槽是"最后一帧"语义 —— 每帧覆盖，永不排队。
    #[test]
    fn fi02_the_latest_slot_holds_one_frame_not_a_queue() {
        let slot: Arc<RwLock<Option<u32>>> = Arc::new(RwLock::new(None));
        for frame_no in 1..=500 {
            store(&slot, frame_no);
        }
        let held = slot.read().unwrap();
        assert_eq!(*held, Some(500), "槽里必须只有最新那一帧");
        // 覆盖写不换指针、也不新增任何容器 ⇒ "不堆积"是结构性的，不是靠清理。
        assert_eq!(slot.read().ok().map(|g| *g), Some(Some(500)));
    }

    /// FI-02 的另一半：读侧/写侧任一时刻崩了，循环不能跟着崩。
    /// `store` 用的是 `if let Ok(..)`，锁中毒时丢掉这一帧继续跑 —— 这条测试盯着这个选择。
    #[test]
    fn fi02_a_poisoned_slot_cannot_take_the_loop_down() {
        let slot: Arc<RwLock<Option<u32>>> = Arc::new(RwLock::new(Some(1)));
        let held = slot.clone();
        let boom = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _guard = held.write().unwrap();
            panic!("注入：持写锁时 panic");
        }));
        assert!(boom.is_err(), "前置条件：槽要处于中毒状态");

        // 关键断言：中毒之后 `store` 只是丢掉这一帧，不 panic（= 循环不退出）。
        store(&slot, 2);
        assert!(
            slot.read().is_err(),
            "中毒的槽读不出来，这正是 `store` 不许 panic 的理由"
        );

        // 换一个健康的槽，恢复路径照常。
        let fresh: Arc<RwLock<Option<u32>>> = Arc::new(RwLock::new(None));
        store(&fresh, 3);
        assert_eq!(*fresh.read().unwrap(), Some(3));
    }

    /// FI-01：规划原文假设"sysinfo refresh 失败返回 Err"，实测这条假设在本项目用的版本上不成立 ——
    /// `sysinfo 0.33.1` 的采集 API 全是 `fn refresh_*/(&mut self)`，没有可注入的错误通道。
    /// 这条测试把那个事实钉住：一旦哪天它开始返回 `Result`，这里会编译失败并逼我们补真正的 FI-01。
    #[test]
    fn fi01_sysinfo_offers_no_error_channel_to_inject() {
        let mut sys = System::new();
        // 显式要求返回类型是 `()`：写成 `let x: Result<..> = ...` 会编不过，那就是本测试要盯的变化。
        let unit: () = sys.refresh_cpu_usage();
        assert_eq!(unit, ());
        let mut disks = Disks::new();
        let unit: () = disks.refresh(true);
        assert_eq!(unit, ());
        // 因此这里能注入的"失败"只有 FI-03（panic 路径）与 FI-04（单块分区拿不到容量），
        // 而"使用上次缓存 + 标 stale"由前端的采集停滞检测承担（>3 个周期无帧）。
        let mut collector = Collector::new();
        let frame = collector.snapshot(Duration::from_millis(0));
        assert!(frame.timestamp_ms > 0, "没有错误通道时，唯一可信的失败信号就是'这一帧根本没出来'");
    }

    // ==================== IT-01~IT-03（06 的集成矩阵）====================

    /// IT-01：事件名是前后端唯一的约定，改名不会有任何编译错误 —— 界面只会永远
    /// "等待采集首帧…"，后端照样在推。这条把三个事件名钉在前端契约文件上。
    #[test]
    fn it01_every_stream_event_name_is_registered_by_the_frontend_contract() {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let contract = std::fs::read_to_string(manifest.join("../src/ipc_contract.ts")).unwrap();
        for event in [METRICS_EVENT, PROCESSES_EVENT, crate::alert::ALERT_EVENT] {
            assert!(
                contract.contains(&format!("\"{event}\"")),
                "前端 MonitorEvent 里没有登记 {event}，推了也没人接"
            );
        }
        // 首帧的结构完整性：payload 少一个字段与事件名错了的表现完全一样（界面静默空白），
        // 所以这两件事在同一条测试里钉。
        let mut collector = Collector::new();
        let frame = collector.snapshot(Duration::from_secs(5));
        let json = serde_json::to_string(&frame).unwrap();
        for key in [
            "timestampMs", "uptimeSeconds", "cpu", "memory", "disks", "networks",
            "total", "perCore", "coreCount", "usagePercent", "pressure",
        ] {
            assert!(json.contains(&format!("\"{key}\"")), "metrics 载荷缺字段 {key}：{json}");
        }
        assert!(!json.contains('_'), "metrics 载荷必须全 camelCase：{json}");
    }

    /// IT-02：改间隔要落在**循环真正读的那份配置**上。`spawn` 走的是 `Arc` 克隆，
    /// 所以这条测的是共享性本身 —— 哪天有人把 `config.clone()` 换成 `RwLock::new(*guard)`，
    /// 现象是"界面把 5 s 显示成生效了，采集照旧 1 s 一帧"，没有任何报错。
    #[test]
    fn it02_a_config_change_lands_on_the_copy_the_loop_reads() {
        let service = MonitorService::new();
        // 与 spawn 完全同款：先克隆一份交给"循环"，再从原对象改配置。
        let loop_side = {
            let cloned = service.clone();
            std::thread::spawn(move || cloned)
        }
        .join()
        .unwrap();

        let requested = MonitorConfig {
            interval_ms: 1,
            disk_interval_ms: 1_000,
            process_interval_ms: 3_000,
            paused: false,
        };
        let effective = service.set_config(requested);
        let seen = loop_side.config.read().map(|g| g.clone()).unwrap_or_default();
        assert_eq!(
            seen.interval_ms, effective.interval_ms,
            "循环那份配置没跟着变 ⇒ 改间隔只改了界面"
        );
        assert_eq!(seen.disk_interval_ms, effective.disk_interval_ms);
        assert_eq!(seen.process_interval_ms, effective.process_interval_ms);
        // 脏输入夹取后回读的是生效值，不是用户填的那个（否则界面会显示一份后端没收的配置）。
        // 下限具体是多少由 `config_clamps_to_safe_ranges` 钉；这里只要求"1 ms 没被照单收下"。
        let dirty = service.set_config(MonitorConfig {
            interval_ms: 1,
            ..Default::default()
        });
        assert!(dirty.interval_ms > 1, "1 ms 这种脏输入必须被夹掉：{dirty:?}");
        assert_eq!(
            loop_side.config.read().unwrap().interval_ms,
            dirty.interval_ms,
            "夹取后的生效值没落到循环那份配置上"
        );
    }

    /// IT-03：关进程流要落在**循环轮询的那面旗**上（同一个 `Arc<AtomicBool>`）。
    /// 表现同上：旗没共享受到，"停止进程枚举"就只是一句界面文案。
    #[test]
    fn it03_stopping_the_stream_flips_the_flag_the_loop_polls() {
        let service = MonitorService::new();
        let loop_side = {
            let cloned = service.clone();
            std::thread::spawn(move || cloned)
        }
        .join()
        .unwrap();

        assert!(!service.process_stream_enabled(), "默认不该开着枚举");
        service.set_process_stream(true);
        assert!(loop_side.process_stream.load(Ordering::Relaxed), "开启没传到循环那份");
        service.set_process_stream(false);
        assert!(!loop_side.process_stream.load(Ordering::Relaxed), "停止没传到循环那份");
        assert!(!service.process_stream_enabled());
    }

    /// IT-02 的另一半：配置必须是**每帧重读**。循环开头读一次的话，改间隔要重启进程才生效，
    /// 而界面上看起来已经生效了。
    #[test]
    fn it02_the_loop_rereads_the_config_every_frame() {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let source = std::fs::read_to_string(manifest.join("src/monitor.rs")).unwrap();
        let loop_body = source
            .split("async fn run_metrics_loop")
            .nth(1)
            .expect("应有 run_metrics_loop")
            .split("\n}\n")
            .next()
            .expect("函数应有结尾")
            .to_string();
        let loop_at = loop_body.find("loop {").expect("循环体应有 loop {");
        let read_at = loop_body
            .find("let cfg = config.read()")
            .expect("循环里没有读配置？");
        assert!(
            read_at > loop_at,
            "配置在 loop 之外被读了 ⇒ 改间隔要重启进程才生效"
        );
        assert!(
            loop_body.contains("let tick = Duration::from_millis(cfg.interval_ms)"),
            "帧间隔没跟着每帧重读出来的配置走"
        );
    }
}
