use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use sysinfo::{CpuRefreshKind, Disks, MemoryRefreshKind, Networks, RefreshKind, System};
use tauri::{AppHandle, Emitter};

pub const METRICS_EVENT: &str = "sys://metrics";
pub const PROCESSES_EVENT: &str = "sys://processes";

const MIN_INTERVAL_MS: u64 = 200;
const MAX_PROCESSES_PER_PAGE: usize = 300;
/// 进程表在此时间内视为可信，避免每次 kill 校验都全量枚举。
const PROCESS_CACHE_TTL: Duration = Duration::from_secs(10);

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
}

impl Collector {
    pub fn new() -> Self {
        let mut sys = System::new_with_specifics(
            RefreshKind::new()
                .with_cpu(CpuRefreshKind::everything())
                .with_memory(MemoryRefreshKind::everything()),
        );
        // 型号与核心数只在首次采样时填充，之后 refresh_cpu_usage 只更新百分比。
        sys.refresh_cpu();

        Self {
            sys,
            networks: Networks::new_with_refreshed_list(),
            disks: Disks::new_with_refreshed_list(),
            last_cpu_sample: Instant::now(),
            last_network_sample: Instant::now(),
            last_disk_sample: Instant::now(),
        }
    }

    pub fn refresh_cpu(&mut self) {
        self.sys.refresh_cpu_usage();
        self.last_cpu_sample = Instant::now();
    }

    pub fn cpu_metrics(&self) -> CpuMetrics {
        let per_core: Vec<f64> = self.sys.cpus().iter().map(|c| c.cpu_usage() as f64).collect();
        let total = if per_core.is_empty() {
            self.sys.global_cpu_info().cpu_usage() as f64
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
        let available = self.sys.available_memory();
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
        self.networks.refresh();
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

    /// 磁盘容量按 `disk_interval_ms` 慢刷，I/O 速率每次算增量。
    pub fn disk_metrics(&mut self, disk_interval: Duration) -> Vec<DiskMetrics> {
        if self.last_disk_sample.elapsed() >= disk_interval {
            self.disks.refresh();
            self.last_disk_sample = Instant::now();
        }

        let elapsed = self.last_cpu_sample.elapsed().as_secs_f64().max(0.001);
        let mut out = Vec::new();
        for disk in self.disks.iter() {
            let total = disk.total_space();
            let available = disk.available_space();
            let used = total.saturating_sub(available);
            let key = disk.mount_point().to_string_lossy().into_owned();

            // sysinfo 0.30 的 Disk 不暴露 read_bytes/written_bytes，速率留空由 UI 显示为“—”。
            let _ = elapsed;
            let (read_rate, write_rate) = (None, None);

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
        out.sort_by(|a, b| b.total_bytes.cmp(&a.total_bytes));
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
            RefreshKind::new()
                .with_cpu(CpuRefreshKind::everything())
                .with_memory(MemoryRefreshKind::everything()),
        );
        sys.refresh_cpu();
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

/// 进程采集独立于指标采集：枚举成本高，单独低频循环。
pub fn collect_processes(sys: &mut System, warmed: bool) -> ProcessPage {
    sys.refresh_processes();

    let mut items: Vec<ProcessInfo> = sys
        .processes()
        .iter()
        .map(|(pid, proc_info)| ProcessInfo {
            pid: pid.as_u32(),
            name: {
                let raw = proc_info.name();
                if raw.is_empty() {
                    "(unknown)".to_string()
                } else {
                    raw.to_string()
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
        .collect();

    items.sort_by(|a, b| {
        b.cpu_usage
            .partial_cmp(&a.cpu_usage)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.memory_bytes.cmp(&a.memory_bytes))
    });

    let total = items.len();
    items.truncate(MAX_PROCESSES_PER_PAGE);

    ProcessPage {
        total,
        items,
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

    let mut sys = System::new();
    sys.refresh_processes();
    let key = sysinfo::Pid::from_u32(pid);
    sys.processes().get(&key).map(|p| ProcessSummary {
        name: {
            let raw = p.name();
            if raw.is_empty() {
                "(unknown)".to_string()
            } else {
                raw.to_string()
            }
        },
        memory_bytes: p.memory(),
    })
}

// ==================== 服务 ====================

/// Tauri managed state：唯一采集器 + 最新帧缓存。
pub struct MonitorService {
    config: Arc<RwLock<MonitorConfig>>,
    latest: Arc<RwLock<Option<MetricsSnapshot>>>,
    latest_processes: Arc<RwLock<Option<ProcessPage>>>,
    process_stream: Arc<AtomicBool>,
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

    pub fn set_process_stream(&self, enabled: bool) {
        self.process_stream.store(enabled, Ordering::Relaxed);
    }

    pub fn process_stream_enabled(&self) -> bool {
        self.process_stream.load(Ordering::Relaxed)
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
    pub fn spawn(&self, app: AppHandle) {
        let config = self.config.clone();
        let latest = self.latest.clone();
        {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                run_metrics_loop(app, config, latest).await;
            });
        }

        let config = self.config.clone();
        let stream = self.process_stream.clone();
        let latest_processes = self.latest_processes.clone();
        tauri::async_runtime::spawn(async move {
            run_process_loop(app, config, stream, latest_processes).await;
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
) {
    let mut collector = Collector::new();
    // 首个采样点只建立基线，不推送（CPU 增量需要前一次采样）。
    collector.refresh_cpu();
    tokio::time::sleep(Duration::from_millis(300)).await;

    loop {
        let cfg = config.read().map(|g| g.clone()).unwrap_or_default();
        if cfg.paused {
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
                let _ = app.emit(METRICS_EVENT, &snapshot);
            }
            Err(_) => {
                // 采集器 panic 后重建，保证循环不退出。
                collector = Collector::new();
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
) {
    let mut sys = System::new();
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
            sys.refresh_processes();
            tokio::time::sleep(Duration::from_millis(350)).await;
            warmed = true;
        }

        let frame = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            collect_processes(&mut sys, warmed)
        }));

        match frame {
            Ok(page) => {
                store(&latest_processes, page.clone());
                let _ = app.emit(PROCESSES_EVENT, &page);
            }
            Err(_) => {
                sys = System::new();
                warmed = false;
            }
        }

        tokio::time::sleep(Duration::from_millis(interval)).await;
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
    fn disk_metrics_have_no_fake_io_when_unsupported() {
        let mut collector = Collector::new();
        let mut seen_total = false;
        for _ in 0..2 {
            let disks = collector.disk_metrics(Duration::from_secs(10));
            for d in &disks {
                seen_total |= d.total_bytes > 0;
                if let Some(read) = d.read_bytes_per_sec {
                    assert!(read >= 0.0);
                }
            }
            std::thread::sleep(Duration::from_millis(220));
        }
        assert!(seen_total);
    }

    #[test]
    fn process_collection_is_sorted_and_truncated() {
        let mut sys = System::new();
        // 两次采样才能得到真实进程 CPU。
        collect_processes(&mut sys, false);
        let page = collect_processes(&mut sys, true);
        assert!(page.total > 5);
        assert!(!page.warming);
        assert!(page.items.len() <= MAX_PROCESSES_PER_PAGE);
        for w in page.items.windows(2) {
            assert!(w[0].cpu_usage >= w[1].cpu_usage, "not sorted by cpu");
        }
        assert!(page.items.iter().all(|p| !p.name.is_empty()));
    }

    #[test]
    fn looks_up_own_process_by_pid() {
        let me = std::process::id();
        let found = lookup_process(me).expect("self pid must resolve");
        assert!(!found.name.is_empty());
        assert!(found.memory_bytes > 0);
        assert!(lookup_process(u32::MAX - 1).is_none());
    }
}
