use crate::cleanup::{
    cleanup_categories, find_large_files, get_startup_items, resolve_scan_path, scan_junk,
    CleanupResult, JunkReport, LargeFile, StartupItem,
};
use crate::error::{AppError, CommandResult};
use crate::monitor::{self, MonitorConfig, MonitorService, ProcessPage, StaticInfo};
use crate::safety::{self, KillOutcome, KillValidation};
use serde::Serialize;
use std::time::{Duration, Instant};
use tauri::State;

const DEFAULT_KILL_GRACE_MS: u64 = 3000;

// ==================== 监控链路 ====================

#[tauri::command]
pub fn get_static_info() -> StaticInfo {
    monitor::collect_static_info()
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetricsResponse {
    pub snapshot: Option<monitor::MetricsSnapshot>,
}

/// 首帧事件到达前的兜底查询。
#[tauri::command]
pub fn get_metrics_snapshot(state: State<'_, MonitorService>) -> MetricsResponse {
    MetricsResponse {
        snapshot: state.latest_metrics(),
    }
}

#[tauri::command]
pub fn set_monitor_config(
    state: State<'_, MonitorService>,
    config: MonitorConfig,
) -> MonitorConfig {
    state.set_config(config)
}

#[tauri::command]
pub fn start_process_stream(state: State<'_, MonitorService>) -> bool {
    state.set_process_stream(true);
    state.process_stream_enabled()
}

#[tauri::command]
pub fn stop_process_stream(state: State<'_, MonitorService>) -> bool {
    state.set_process_stream(false);
    state.process_stream_enabled()
}

/// 优先读采集循环的缓存；冷启动时做一次两次采样的兜底枚举。
#[tauri::command]
pub fn get_processes(state: State<'_, MonitorService>) -> ProcessPage {
    if let Some(page) = state.latest_processes() {
        return page;
    }

    let mut sys = sysinfo::System::new();
    sys.refresh_processes();
    let started = Instant::now();
    while started.elapsed() < Duration::from_millis(260) {
        std::thread::sleep(Duration::from_millis(20));
    }
    monitor::collect_processes(&mut sys, true)
}

// ==================== 危险操作 ====================

#[tauri::command]
pub fn validate_kill(pid: u32) -> CommandResult<KillValidation> {
    safety::validate_kill(pid)
}

#[tauri::command]
pub async fn kill_process(pid: u32, grace_ms: Option<u64>) -> CommandResult<KillOutcome> {
    safety::terminate(pid, Duration::from_millis(grace_ms.unwrap_or(DEFAULT_KILL_GRACE_MS))).await
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DnsFlushResult {
    pub flushed: bool,
    pub message: String,
    /// 未能自动刷新时给出用户可自行执行的命令；应用内绝不调用 sudo。
    pub manual_command: Option<String>,
}

/// 只使用用户态可执行的命令；失败时返回可复制的手动命令，而不是挂起等密码。
#[tauri::command]
pub fn flush_dns_cache() -> DnsFlushResult {
    let (program, args, manual): (&str, Vec<&str>, &str) = if cfg!(target_os = "macos") {
        (
            "dscacheutil",
            vec!["-flushcache"],
            "sudo dscacheutil -flushcache; sudo killall -HUP mDNSResponder",
        )
    } else if cfg!(target_os = "linux") {
        (
            "resolvectl",
            vec!["flush-caches"],
            "sudo resolvectl flush-caches",
        )
    } else if cfg!(windows) {
        ("ipconfig", vec!["/flushdns"], "ipconfig /flushdns")
    } else {
        return DnsFlushResult {
            flushed: false,
            message: "当前平台不支持刷新 DNS 缓存".to_string(),
            manual_command: None,
        };
    };

    match std::process::Command::new(program).args(&args).output() {
        Ok(out) if out.status.success() => DnsFlushResult {
            flushed: true,
            message: "DNS 缓存已刷新".to_string(),
            manual_command: None,
        },
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            let hint = if stderr.contains("not found") {
                "sudo systemd-resolve --flush-caches"
            } else {
                manual
            };
            DnsFlushResult {
                flushed: false,
                message: if stderr.is_empty() {
                    "DNS 缓存刷新未生效".to_string()
                } else {
                    crate::log_sanitize::sanitize(&format!(
                        "DNS 缓存刷新未生效: {}",
                        stderr
                    ))
                },
                manual_command: Some(hint.to_string()),
            }
        }
        Err(e) => DnsFlushResult {
            flushed: false,
            message: crate::log_sanitize::sanitize(&format!("无法调用 {}: {}", program, e)),
            manual_command: Some(manual.to_string()),
        },
    }
}

// ==================== 系统维护 ====================

#[tauri::command]
pub fn scan_junk_files() -> JunkReport {
    scan_junk()
}

#[tauri::command]
pub fn cleanup_junk_files(ids: Vec<String>) -> CommandResult<CleanupResult> {
    if ids.is_empty() {
        return Err(AppError::invalid_input("请至少选择一个清理类别"));
    }
    Ok(cleanup_categories(&ids))
}

#[tauri::command]
pub fn find_large_files_cmd(
    path: Option<String>,
    min_size_mb: u64,
    limit: Option<usize>,
) -> CommandResult<Vec<LargeFile>> {
    if min_size_mb == 0 || min_size_mb > 1024 * 1024 {
        return Err(AppError::invalid_input("最小文件大小需在 1MB ~ 1TB 之间"));
    }
    let root = resolve_scan_path(path.as_deref())?;
    Ok(find_large_files(
        &root,
        min_size_mb * 1024 * 1024,
        limit.unwrap_or(50).clamp(1, 500),
    ))
}

#[tauri::command]
pub fn get_startup_items_cmd() -> Vec<StartupItem> {
    get_startup_items()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dns_flush_never_shells_out_to_sudo() {
        let result = flush_dns_cache();
        if let Some(cmd) = &result.manual_command {
            assert!(!result.flushed, "自动刷新成功时不应再给手动命令");
        }
        assert!(!result.message.contains("sudo: a terminal is required"));
    }

    #[test]
    fn large_file_scan_rejects_out_of_range_sizes() {
        assert_eq!(
            find_large_files_cmd(None, 0, None).unwrap_err().code,
            "INVALID_INPUT"
        );
        assert_eq!(
            find_large_files_cmd(None, 2_000_000, None).unwrap_err().code,
            "INVALID_INPUT"
        );
    }

    #[test]
    fn large_file_scan_expands_home_alias() {
        let files = find_large_files_cmd(Some("~".to_string()), 4_096, Some(1)).unwrap();
        assert!(files.iter().all(|f| !f.path.starts_with('~')));
    }

    #[test]
    fn cleanup_requires_selection() {
        assert_eq!(
            cleanup_junk_files(vec![]).unwrap_err().code,
            "INVALID_INPUT"
        );
    }
}
