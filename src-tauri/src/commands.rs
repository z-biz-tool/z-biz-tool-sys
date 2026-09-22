use crate::cleanup::{
    cleanup_categories, find_large_files, get_startup_items, junk_scan_flags,
    request_cancel_junk_scan, resolve_scan_path, scan_junk_with_progress, CleanupResult, JunkReport,
    LargeFile, StartupItem,
};
use crate::error::{AppError, CommandResult};
use crate::monitor::{
    self, MonitorConfig, MonitorService, ProcessPage, ProcessQuery, StaticInfo,
};
use crate::prefs;
use crate::safety::{self, KillOutcome, KillValidation};
use serde::Serialize;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, State};

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

/// 告警阈值配置（T5-01）：与 `set_monitor_config` 同口径 —— 后端夹取并把真正生效的值回给前端，
/// 界面上的输入框据此校正，避免出现"显示 150、实际按 100 判"的两套数。
#[tauri::command]
pub fn get_alert_config(state: State<'_, crate::alert::AlertState>) -> crate::alert::AlertConfig {
    state.get()
}

#[tauri::command]
pub fn set_alert_config(
    state: State<'_, crate::alert::AlertState>,
    config: crate::alert::AlertConfig,
) -> crate::alert::AlertConfig {
    state.set(config)
}

/// 历史趋势（T3-07）：读已落盘的 10 s 采样点，跨度超过保留窗口时按上限夹取。
/// 存储不可用时返回错误而不是空页 —— 前端需要能区分"没有历史"和"存不了历史"。
#[tauri::command]
pub fn get_history(
    state: State<'_, crate::history::HistoryState>,
    span_seconds: Option<u64>,
) -> CommandResult<crate::history::HistoryPage> {
    let Some(store) = state.0.as_ref() else {
        return Err(AppError::failed("历史存储不可用：应用数据目录无法写入"));
    };
    let span = span_seconds.unwrap_or(crate::history::DEFAULT_SPAN_SECS);
    Ok(crate::history::query(store, crate::history::now_ms(), span))
}

/// 告警历史（T5-03）：读已落盘的触发记录，默认最近 7 天、上界为保留窗口。
/// 与 `get_history` 同口径 —— 存储不可用时返回错误，前端要能分辨"没发生过告警"和"存不下"。
#[tauri::command]
pub fn get_alert_history(
    state: State<'_, crate::history::AlertHistoryState>,
    span_seconds: Option<u64>,
) -> CommandResult<crate::history::AlertHistoryPage> {
    let Some(store) = state.0.as_ref() else {
        return Err(AppError::failed(
            "告警历史存储不可用：应用数据目录无法写入，当前只显示本次会话内的告警",
        ));
    };
    let span = span_seconds.unwrap_or(crate::history::DEFAULT_ALERT_SPAN_SECS);
    Ok(crate::history::query_alerts(
        store,
        crate::history::now_ms(),
        span,
    ))
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

/// 前端改动过滤/排序/分页时调用；下一帧（≤ process_interval）生效。
/// 返回归一化后的查询，前端据此校正越界的 limit/offset。
#[tauri::command]
pub fn set_process_query(
    state: State<'_, MonitorService>,
    query: ProcessQuery,
) -> ProcessQuery {
    state.set_process_query(query)
}

/// 优先读采集循环的缓存；冷启动时做一次两次采样的兜底枚举。
#[tauri::command]
pub fn get_processes(state: State<'_, MonitorService>) -> ProcessPage {
    let query = state.current_process_query();
    // 缓存可能还是上一次查询（关键字/分页）的帧，落后时必须重采集。
    if state.processes_match_query() {
        if let Some(page) = state.latest_processes() {
            return page;
        }
    }

    let mut sys = sysinfo::System::new();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
    let started = Instant::now();
    while started.elapsed() < Duration::from_millis(260) {
        std::thread::sleep(Duration::from_millis(20));
    }
    monitor::collect_processes(&mut sys, true, &query)
}

/// 进程详情（点击进程行时按需读取，不进 3s 事件流）。
#[tauri::command]
pub fn get_process_detail(pid: u32) -> CommandResult<monitor::ProcessDetail> {
    monitor::collect_process_detail(pid)
        .ok_or_else(|| AppError::process_not_found(format!("PID {pid} 不存在或已退出")))
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

/// 扫描在 blocking 线程上跑，逐目录把进度推给前端（T3-09）。
/// 之前它是同步 command：整段目录遍历期间前端只能看着按钮转圈，也无法取消。
#[tauri::command]
pub async fn scan_junk_files(app: AppHandle) -> CommandResult<JunkReport> {
    // 认领放在 spawn 之前：否则从"命令下发"到"扫描线程真正起跑"之间的窗口里，
    // 用户点取消会因为这一轮还没被认领而落空，扫描照旧跑完。
    let Some(guard) = junk_scan_flags().begin() else {
        return Err(AppError::invalid_input("已有一轮扫描在进行中，请先取消它"));
    };
    let joined = tauri::async_runtime::spawn_blocking(move || {
        scan_junk_with_progress(
            &mut |frame| {
                let _ = app.emit(crate::cleanup::SCAN_PROGRESS_EVENT, frame);
            },
            &guard,
        )
    })
    .await;
    match joined {
        Ok(report) => Ok(report),
        Err(e) => Err(AppError::failed(format!("扫描线程异常退出：{e}"))),
    }
}

/// 取消进行中的扫描；返回 false 表示当前根本没有扫描在跑。
#[tauri::command]
pub fn cancel_junk_scan() -> bool {
    request_cancel_junk_scan()
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

// ==================== 偏好导入/导出（T5-11）====================

#[tauri::command]
pub fn export_prefs_file(
    path: String,
    prefs: serde_json::Value,
) -> CommandResult<prefs::ExportOutcome> {
    prefs::write_prefs_file(&path, prefs)
}

#[tauri::command]
pub fn import_prefs_file(path: String) -> CommandResult<prefs::ImportOutcome> {
    prefs::read_prefs_file(&path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dns_flush_never_shells_out_to_sudo() {
        let result = flush_dns_cache();
        if let Some(cmd) = &result.manual_command {
            assert!(!result.flushed, "自动刷新成功时不应再给手动命令");
            assert!(!cmd.is_empty(), "手动兜底命令不应为空");
        }
        assert!(!result.message.contains("sudo: a terminal is required"));
    }

    /// SEC-V06：后端不得以提权方式执行命令，失败时只能返回供用户复制的手动命令。
    #[test]
    fn backend_never_constructs_a_sudo_command() {
        let src_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        for entry in std::fs::read_dir(&src_dir).expect("src 目录应存在") {
            let path = entry.expect("可读目录项").path();
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("源码可读");
            if text.contains("Command::new(\"sudo\")") || text.contains("Command::new(\"su\")") {
                offenders.push(path.file_name().unwrap().to_string_lossy().into_owned());
            }
        }
        assert!(offenders.is_empty(), "发现提权调用：{offenders:?}");
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
    fn home_alias_expands_to_a_real_path() {
        let expanded = resolve_scan_path(Some("~")).unwrap();
        assert_eq!(expanded, dirs::home_dir().expect("测试机应有用户目录"));
    }

    #[test]
    fn large_file_scan_returns_only_files_over_threshold() {
        // 用临时目录验证，避免遍历真实用户目录拖慢测试
        let dir = std::env::temp_dir().join(format!("zsys-large-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("big.bin"), vec![7u8; 3 * 1024 * 1024]).unwrap();
        std::fs::write(dir.join("small.bin"), vec![7u8; 1024]).unwrap();

        let files = find_large_files(&dir, 1024 * 1024, 10);
        let names: Vec<&str> = files.iter().map(|f| f.path.as_str()).map(|p| {
            std::path::Path::new(p).file_name().and_then(|n| n.to_str()).unwrap_or("")
        }).collect();
        assert!(names.contains(&"big.bin"), "应找到 big.bin，实际 {names:?}");
        assert!(!names.contains(&"small.bin"), "小文件不应出现在结果中");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cleanup_requires_selection() {
        assert_eq!(
            cleanup_junk_files(vec![]).unwrap_err().code,
            "INVALID_INPUT"
        );
    }
}
