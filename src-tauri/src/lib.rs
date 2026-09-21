use serde::{Deserialize, Serialize};
use sysinfo::{System, Disks, Networks};

mod cleanup;
use cleanup::{cleanup_categories, find_large_files, get_startup_items, scan_junk, CleanupResult, JunkReport, LargeFile, StartupItem};

// 系统信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemInfo {
    pub hostname: String,
    pub os: String,
    pub kernel: String,
    pub uptime: String,
    pub cpu_model: String,
    pub cpu_cores: usize,
    pub total_memory: f64,
    pub used_memory: f64,
    pub disk_total: f64,
    pub disk_used: f64,
    pub network_interfaces: Vec<NetworkInterface>,
}

// 网络接口
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkInterface {
    pub name: String,
    pub ip: String,
    pub mac: String,
    pub speed: u64,
}

// CPU 信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CpuInfo {
    pub usage: f32,
    pub cores: Vec<CpuCore>,
}

// CPU 核心
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CpuCore {
    pub id: usize,
    pub usage: f32,
}

// 内存信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryInfo {
    pub total: f64,
    pub used: f64,
    pub free: f64,
    pub usage_percent: f64,
}

// 磁盘信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskInfo {
    pub name: String,
    pub mount_point: String,
    pub total: f64,
    pub used: f64,
    pub free: f64,
    pub usage_percent: f64,
}

// 网络统计
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkStats {
    pub interface: String,
    pub bytes_received: u64,
    pub bytes_sent: u64,
    pub packets_received: u64,
    pub packets_sent: u64,
}

// 进程信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
    pub cpu_usage: f32,
    pub memory_usage: u64,
    pub threads: usize,
}

// 获取系统信息

pub async fn get_system_info() -> Result<SystemInfo, String> {
    let mut sys = System::new_all();
    sys.refresh_all();

    let hostname = System::host_name().unwrap_or_else(|| "Unknown".to_string());
    let os = System::long_os_version().unwrap_or_else(|| "Unknown".to_string());
    let kernel = System::os_version().unwrap_or_else(|| "Unknown".to_string());
    let uptime = System::uptime();

    // 格式化运行时间
    let days = uptime / 86400;
    let hours = (uptime % 86400) / 3600;
    let minutes = (uptime % 3600) / 60;
    let uptime_str = format!("{}天 {}小时 {}分钟", days, hours, minutes);

    // CPU 信息
    let cpu_model = sys.cpus().first().map(|c| c.brand().to_string()).unwrap_or_else(|| "Unknown".to_string());
    let cpu_cores = sys.cpus().len();

    // 内存信息
    let total_memory = sys.total_memory() as f64 / 1024.0 / 1024.0 / 1024.0;
    let used_memory = sys.used_memory() as f64 / 1024.0 / 1024.0 / 1024.0;

    // 磁盘信息
    let mut disk_total = 0.0;
    let mut disk_used = 0.0;
    let disks = Disks::new_with_refreshed_list();
    for disk in disks.list() {
        disk_total += disk.total_space() as f64 / 1024.0 / 1024.0 / 1024.0;
        disk_used += (disk.total_space() - disk.available_space()) as f64 / 1024.0 / 1024.0 / 1024.0;
    }

    // 网络接口
    let networks = Networks::new_with_refreshed_list();
    let network_interfaces: Vec<NetworkInterface> = networks.list().iter().map(|(name, _data)| {
        NetworkInterface {
            name: name.clone(),
            ip: String::new(),
            mac: "AA:BB:CC:DD:EE:FF".to_string(),
            speed: 1000,
        }
    }).collect();

    Ok(SystemInfo {
        hostname,
        os,
        kernel,
        uptime: uptime_str,
        cpu_model,
        cpu_cores,
        total_memory,
        used_memory,
        disk_total,
        disk_used,
        network_interfaces,
    })
}

// 获取 CPU 使用率

pub async fn get_cpu_usage() -> Result<CpuInfo, String> {
    let mut sys = System::new();
    sys.refresh_cpu();

    let usage = sys.global_cpu_info().cpu_usage();
    let cores: Vec<CpuCore> = sys.cpus().iter().enumerate().map(|(i, cpu)| {
        CpuCore {
            id: i,
            usage: cpu.cpu_usage(),
        }
    }).collect();

    Ok(CpuInfo { usage, cores })
}

// 获取内存信息

pub async fn get_memory_info() -> Result<MemoryInfo, String> {
    let mut sys = System::new();
    sys.refresh_memory();

    let total = sys.total_memory() as f64 / 1024.0 / 1024.0 / 1024.0;
    let used = sys.used_memory() as f64 / 1024.0 / 1024.0 / 1024.0;
    let free = sys.available_memory() as f64 / 1024.0 / 1024.0 / 1024.0;
    let usage_percent = (used / total) * 100.0;

    Ok(MemoryInfo {
        total,
        used,
        free,
        usage_percent,
    })
}

// 获取磁盘信息

pub async fn get_disk_info() -> Result<Vec<DiskInfo>, String> {
    let disks = Disks::new_with_refreshed_list();

    let result: Vec<DiskInfo> = disks.list().iter().map(|disk| {
        let total = disk.total_space() as f64 / 1024.0 / 1024.0 / 1024.0;
        let used = (disk.total_space() - disk.available_space()) as f64 / 1024.0 / 1024.0 / 1024.0;
        let free = disk.available_space() as f64 / 1024.0 / 1024.0 / 1024.0;
        let usage_percent = (used / total) * 100.0;

        DiskInfo {
            name: disk.name().to_string(),
            mount_point: disk.mount_point().to_string(),
            total,
            used,
            free,
            usage_percent,
        }
    }).collect();

    Ok(result)
}

// 获取网络统计

pub async fn get_network_stats() -> Result<Vec<NetworkStats>, String> {
    let networks = Networks::new_with_refreshed_list();

    let stats: Vec<NetworkStats> = networks.list().iter().map(|(name, data)| {
        NetworkStats {
            interface: name.clone(),
            bytes_received: data.total_received(),
            bytes_sent: data.total_transmitted(),
            packets_received: 0,
            packets_sent: 0,
        }
    }).collect();

    Ok(stats)
}

// 获取进程列表

pub async fn get_processes() -> Result<Vec<ProcessInfo>, String> {
    let mut sys = System::new();
    sys.refresh_processes();

    let mut processes: Vec<ProcessInfo> = sys.processes().iter().map(|(pid, proc_info)| {
        ProcessInfo {
            pid: pid.as_u32(),
            name: proc_info.name().to_string(),
            cpu_usage: proc_info.cpu_usage(),
            memory_usage: proc_info.memory(),
            threads: 0,
        }
    }).collect();

    // 按 CPU 使用率排序
    processes.sort_by(|a, b| b.cpu_usage.partial_cmp(&a.cpu_usage).unwrap_or(std::cmp::Ordering::Equal));

    Ok(processes)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .invoke_handler(tauri::generate_handler![
            get_system_info,
            get_cpu_usage,
            get_memory_info,
            get_disk_info,
            get_network_stats,
            get_processes,
            scan_junk_files,
            cleanup_junk_files,
            find_large_files_cmd,
            get_startup_items_cmd,
            kill_process,
            flush_dns_cache,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

// ================== 系统清理命令 ==================


pub async fn scan_junk_files() -> Result<JunkReport, String> {
    Ok(scan_junk())
}


pub async fn cleanup_junk_files(ids: Vec<String>) -> Result<CleanupResult, String> {
    Ok(cleanup_categories(&ids))
}


pub async fn find_large_files_cmd(
    path: String,
    min_size_mb: u64,
    limit: usize,
) -> Result<Vec<LargeFile>, String> {
    Ok(find_large_files(&path, min_size_mb * 1024 * 1024, limit))
}


pub async fn get_startup_items_cmd() -> Result<Vec<StartupItem>, String> {
    Ok(get_startup_items())
}


pub async fn kill_process(pid: u32) -> Result<bool, String> {
    #[cfg(unix)]
    {
        let result = std::process::Command::new("kill")
            .arg("-9")
            .arg(pid.to_string())
            .output();
        match result {
            Ok(out) => Ok(out.status.success()),
            Err(e) => Err(format!("杀进程失败: {}", e)),
        }
    }
    #[cfg(windows)]
    {
        let result = std::process::Command::new("taskkill")
            .args(["/F", "/PID", &pid.to_string()])
            .output();
        match result {
            Ok(out) => Ok(out.status.success()),
            Err(e) => Err(format!("杀进程失败: {}", e)),
        }
    }
}


pub async fn flush_dns_cache() -> Result<String, String> {
    #[cfg(target_os = "macos")]
    {
        let result = std::process::Command::new("sudo")
            .args(["dscacheutil", "-flushcache"])
            .output();
        match result {
            Ok(out) => Ok(format!("macOS DNS 缓存已刷新: {}", String::from_utf8_lossy(&out.stdout))),
            Err(e) => Err(format!("刷新失败: {}", e)),
        }
    }
    #[cfg(target_os = "linux")]
    {
        // 尝试 systemd-resolved
        let result = std::process::Command::new("sudo")
            .args(["systemctl", "restart", "systemd-resolved"])
            .output();
        match result {
            Ok(out) => Ok(format!("systemd-resolved 已重启: {}", String::from_utf8_lossy(&out.stdout))),
            Err(e) => Err(format!("刷新失败: {}", e)),
        }
    }
    #[cfg(target_os = "windows")]
    {
        let result = std::process::Command::new("ipconfig")
            .arg("/flushdns")
            .output();
        match result {
            Ok(out) => Ok(format!("Windows DNS 缓存已刷新: {}", String::from_utf8_lossy(&out.stdout))),
            Err(e) => Err(format!("刷新失败: {}", e)),
        }
    }
}