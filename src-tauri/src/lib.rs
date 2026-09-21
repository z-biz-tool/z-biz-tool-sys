use serde::{Deserialize, Serialize};

mod cleanup;
mod commands;

// ================== 数据结构定义 ==================

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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .invoke_handler(tauri::generate_handler![
            commands::get_system_info,
            commands::get_cpu_usage,
            commands::get_memory_info,
            commands::get_disk_info,
            commands::get_network_stats,
            commands::get_processes,
            commands::scan_junk_files,
            commands::cleanup_junk_files,
            commands::find_large_files_cmd,
            commands::get_startup_items_cmd,
            commands::kill_process,
            commands::flush_dns_cache,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
