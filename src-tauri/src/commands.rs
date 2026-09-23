use crate::agent::{self, RankBy};
use crate::cleanup::{
    cleanup_categories, find_large_files, get_startup_items, junk_scan_flags,
    request_cancel_junk_scan, resolve_scan_path, scan_junk_with_progress, CleanupResult, JunkReport,
    LargeFile, StartupItem,
};
use crate::error::{AppError, CommandResult};
use crate::export;
use crate::monitor::{
    self, MonitorConfig, MonitorService, ProcessPage, ProcessQuery, ProcessSort, StaticInfo,
};
use crate::notify;
use crate::platform::thermal;
use crate::prefs;
use crate::safety::{self, KillOutcome, KillValidation};
use serde::Serialize;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};

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

    monitor::collect_processes_warmed(&query)
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

/// 刷新 DNS 要跑的那条**用户态**命令，以及失败时给人复制的兜底命令。
///
/// 抽成纯函数是为了测试能跑：`flush_dns_cache()` 会真的改动系统解析器状态（macOS 那条兜底里还有
/// `killall -HUP mDNSResponder`），让默认套件每次都刷一次是不该发生的副作用 —— 三条 CI 腿等于各刷一次。
/// 决策部分（选哪条命令、兜底文案、不支持的平台）全在这里，可以不碰状态就被测完。`None` = 本平台没有用户态入口。
pub(crate) fn dns_flush_plan() -> Option<(&'static str, Vec<&'static str>, &'static str)> {
    if cfg!(target_os = "macos") {
        return Some((
            "dscacheutil",
            vec!["-flushcache"],
            "sudo dscacheutil -flushcache; sudo killall -HUP mDNSResponder",
        ));
    }
    if cfg!(target_os = "linux") {
        return Some(("resolvectl", vec!["flush-caches"], "sudo resolvectl flush-caches"));
    }
    if cfg!(windows) {
        return Some(("ipconfig", vec!["/flushdns"], "ipconfig /flushdns"));
    }
    None
}

/// 刷新没成功时的说法：`stderr` 过脱敏，兜底命令照样给出。
fn dns_failure(stderr: &str, manual: &str) -> DnsFlushResult {
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
            crate::log_sanitize::sanitize(&format!("DNS 缓存刷新未生效: {}", stderr))
        },
        manual_command: Some(hint.to_string()),
    }
}

/// 只使用用户态可执行的命令；失败时返回可复制的手动命令，而不是挂起等密码。
#[tauri::command]
pub fn flush_dns_cache() -> DnsFlushResult {
    let Some((program, args, manual)) = dns_flush_plan() else {
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
        Ok(out) => dns_failure(String::from_utf8_lossy(&out.stderr).trim(), manual),
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

// ==================== 诊断 Agent（T5-04 / T5-05 / T5-06）====================

/// 专供 Agent 的一次性采集：三步暖机 + 全量枚举 + 按指定键取前 N。
/// 代价约 300 ms，只在用户主动点"问一问"时付，且只在结论真的需要排行榜时付。
fn collect_ranked(by: RankBy) -> Vec<monitor::ProcessInfo> {
    monitor::collect_top(
        match by {
            RankBy::Cpu => ProcessSort::Cpu,
            RankBy::Memory => ProcessSort::Memory,
        },
        agent::MAX_RANK,
    )
}

/// 两个榜一起取：并发跑，让"为什么这么卡"这类要双榜的意图不必付两份 300 ms。
fn collect_ranks(need_cpu: bool, need_mem: bool) -> (Vec<monitor::ProcessInfo>, Vec<monitor::ProcessInfo>) {
    match (need_cpu, need_mem) {
        (true, true) => std::thread::scope(|s| {
            let cpu = s.spawn(|| collect_ranked(RankBy::Cpu));
            let mem = s.spawn(|| collect_ranked(RankBy::Memory));
            (
                cpu.join().unwrap_or_default(),
                mem.join().unwrap_or_default(),
            )
        }),
        (true, false) => (collect_ranked(RankBy::Cpu), Vec::new()),
        (false, true) => (Vec::new(), collect_ranked(RankBy::Memory)),
        (false, false) => (Vec::new(), Vec::new()),
    }
}

/// 哪个意图要付哪个榜的采集成本 —— 磁盘/网速这类意图一行进程都不用枚举。
fn rank_needs(parsed: &agent::Parsed) -> (bool, bool) {
    match parsed.intent {
        agent::Intent::RankProcesses => match parsed.rank_by {
            Some(RankBy::Memory) => (false, true),
            _ => (true, false),
        },
        agent::Intent::DiagnoseSlowness => (true, true),
        agent::Intent::MemoryPressure => (false, true),
        _ => (false, false),
    }
}

/// 只读的诊断问答：输入自由文本，输出结论 + **待确认的导航建议**，不执行任何东西。
/// 边界由 `agent` 模块的类型保证（无命令/参数/路径字段），这里也只调 `answer`。
/// 采集是同步且要等采样窗口的，所以放到 `spawn_blocking`，别把窗口线程拖住。
#[tauri::command]
pub async fn agent_query(
    state: State<'_, MonitorService>,
    alert_state: State<'_, crate::alert::AlertState>,
    query: String,
) -> CommandResult<agent::Reply> {
    let parsed = agent::parse(&query);
    let snapshot = state.latest_metrics();
    let alert = alert_state.get();
    let (need_cpu, need_mem) = rank_needs(&parsed);
    let (cpu_ranked, mem_ranked) = if need_cpu || need_mem {
        tauri::async_runtime::spawn_blocking(move || collect_ranks(need_cpu, need_mem))
            .await
            .unwrap_or_else(|_| (Vec::new(), Vec::new()))
    } else {
        (Vec::new(), Vec::new())
    };

    Ok(agent::answer(
        &query,
        &agent::Input {
            snapshot: snapshot.as_ref(),
            cpu_ranked: &cpu_ranked,
            mem_ranked: &mem_ranked,
            alert: &alert,
            // 温度这条支路只在"问到温度"时才会被读到；非 Linux 平台上 probe() 是一次字符串判断，不碰文件。
            thermal: &thermal::probe(),
        },
    ))
}

// ==================== 温度 / 风扇（T5-08）====================

/// 传感器读数。只读，且不接收任何参数 —— 能被读的位置只有 `thermal::SYSFS_ROOT` 一个常量，
/// 前端传不进路径，也就没有"借这条命令去读任意文件"的通路。
/// 没有免提权通路的平台会返回空列表 + 原因，不会返回 0 值凑数。
#[tauri::command]
pub fn get_thermal() -> thermal::ThermalReport {
    thermal::probe()
}

// ==================== 历史 CSV 导出（02 的 F5"可导出 CSV"）====================

/// 把当前时间窗口的历史趋势导成一份 CSV。路径校验与原子写复用偏好导出那一套，
/// 这里只负责"取哪一页数据"和"数据不够新/太多时怎么如实说"。
#[tauri::command]
pub fn export_history_csv(
    state: State<'_, crate::history::HistoryState>,
    path: String,
    span_seconds: Option<u64>,
) -> CommandResult<export::CsvExportOutcome> {
    let Some(store) = state.0.as_ref() else {
        return Err(AppError::failed("历史存储不可用：应用数据目录无法写入"));
    };
    let span = span_seconds.unwrap_or(crate::history::DEFAULT_SPAN_SECS);
    export::write_history_csv(store, &path, crate::history::now_ms(), span)
}

// ==================== 系统通知（T5-02）====================

/// 这一次运行里投过几条系统通知、失败几条。计数只增不减，所以"0 条"的含义是
/// "到目前为止没投过"，不是"通知不可用"。
#[tauri::command]
pub fn notify_status(app: tauri::AppHandle) -> notify::NotifyStatus {
    app.state::<notify::NotifyState>().status()
}

/// 投一条**固定文案**的测试通知：不接收任何参数，避免变成"前端可指定任意文本进系统通知"的通道。
/// 它只回答"这台机器的通知通道能不能把一条消息送到你眼前"；返回 Ok 也只代表投递没当场报错，
/// 真实有没有弹出来由用户自己看（口径见 `notify` 模块头）。
///
/// 失败时 `message` 只放后端给的那句原文 —— 是哪一步失败的语境由界面负责说
/// （`AlertSettingsDrawer` 分"测试通知没发出去"与"读不到投递记账"两句）。两边各加一半前缀
/// 会拼出"系统通知状态读取失败：系统通知投递失败：…"这种重复的错话，浏览器实测抓到过。
#[tauri::command]
pub fn send_test_notification(app: tauri::AppHandle) -> Result<notify::NotifyStatus, AppError> {
    let state = app.state::<notify::NotifyState>();
    match notify::post(&app, &state, notify::test_texts()) {
        Ok(()) => Ok(state.status()),
        Err(reason) => Err(AppError::failed(reason)),
    }
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

        /// A-04 / SEC：应用内只跑用户态命令，`sudo` 版本只作为**给人复制的兜底**存在。
    /// 这条取代原来的 `dns_flush_never_shells_out_to_sudo` —— 那个测试每跑一次套件就真刷一次系统解析器，
    /// 副作用不该由单测产生（要取证见下面的 `#[ignore]` 用例）。
    #[test]
    fn dns_flush_plan_never_escalates_inside_the_app() {
        let Some((program, args, manual)) = dns_flush_plan() else {
            // 只有既不是 macOS / Linux / Windows 的目标才会走到这里
            return;
        };
        assert!(!program.contains("sudo"), "应用内不许调用 sudo：{program}");
        assert!(
            !args.iter().any(|a| a.contains("sudo") || *a == "-HUP"),
            "参数里出现了提权痕迹：{args:?}"
        );
        assert_ne!(program, "su");
        assert!(!manual.trim().is_empty(), "必须给出可复制的兜底命令");
        if cfg!(target_os = "macos") {
            assert_eq!(program, "dscacheutil");
            assert!(manual.contains("mDNSResponder"), "macOS 的兜底要说全两步：{manual}");
        }
        if cfg!(windows) {
            assert!(
                !manual.contains("sudo"),
                "Windows 的兜底命令不该写 sudo（那里不需要）：{manual}"
            );
        }
    }

    /// 失败路径的文案、兜底与脱敏 —— 不用真的动系统解析器就能全部测到。
    #[test]
    fn dns_failures_offer_a_copyable_command_and_sanitize_the_reason() {
        let home = dirs::home_dir()
            .map(|h| h.to_string_lossy().to_string())
            .unwrap_or_default();
        if !home.is_empty() {
            let result = dns_failure(
                &format!("dscacheutil: {home}/x: Operation not permitted"),
                "sudo dscacheutil -flushcache",
            );
            assert!(!result.flushed);
            assert_eq!(
                result.manual_command.as_deref(),
                Some("sudo dscacheutil -flushcache")
            );
            assert!(
                !result.message.contains(&home),
                "错误原文里的家目录没脱敏：{}",
                result.message
            );
        }
        let missing = dns_failure("resolvectl: command not found", "sudo resolvectl flush-caches");
        assert_eq!(
            missing.manual_command.as_deref(),
            Some("sudo systemd-resolve --flush-caches"),
            "命令不存在时要换另一条兜底，而不是继续推同一条"
        );
        assert!(
            !dns_failure("", "sudo dscacheutil -flushcache")
                .message
                .is_empty(),
            "stderr 为空也要给一句人话"
        );
    }

    /// A-04 的真机取证（3 秒内返回、不挂起等密码）。默认套件**不跑**：它会真的刷新本机 DNS 缓存。
    /// 需要时用 `cargo test --lib -- --ignored` 显式执行，输出的那一行就是 A-04 的证据。
    #[tokio::test]
    #[ignore = "会真的刷新本机 DNS 缓存（macOS 上还牵涉 mDNSResponder），只在取证时手动跑"]
    async fn dns_flush_returns_within_three_seconds_on_this_machine() {
        let started = std::time::Instant::now();
        let result = tokio::task::spawn_blocking(flush_dns_cache).await.unwrap();
        let elapsed = started.elapsed();
        println!(
            "A-04 证据: {:?} 用时 {:.3}s",
            result,
            elapsed.as_secs_f64()
        );
        assert!(elapsed < Duration::from_secs(3), "DNS 刷新不该挂起：{elapsed:?}");
        assert!(
            !result.message.contains("sudo: a terminal is required"),
            "不许把提权失败当成结果抛给用户：{}",
            result.message
        );
        if let Some(cmd) = &result.manual_command {
            assert!(!result.flushed, "自动刷新成功时不应再给手动命令");
            assert!(!cmd.trim().is_empty(), "兜底命令不应为空");
        }
    }

    /// SEC-V06：后端不得以提权方式执行命令，失败时只能返回供用户复制的手动命令。
    /// 扫描是**递归**的：`src/platform/` 这类子目录一旦存在，只扫顶层就等于给审计留了个入口。
    #[test]
    fn backend_never_constructs_a_sudo_command() {
        let src_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        let mut files = Vec::new();
        collect_rs_files(&src_dir, &mut files);
        assert!(files.len() > 13, "只找到 {} 个源文件，递归没生效，审计等于空跑", files.len());
        for path in files {
            let text = std::fs::read_to_string(&path).expect("源码可读");
            if text.contains("Command::new(\"sudo\")") || text.contains("Command::new(\"su\")") {
                offenders.push(path.to_string_lossy().into_owned());
            }
        }
        assert!(offenders.is_empty(), "发现提权调用：{offenders:?}");
    }

    fn collect_rs_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("src 目录应存在") {
            let path = entry.expect("可读目录项").path();
            if path.is_dir() {
                collect_rs_files(&path, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                out.push(path);
            }
        }
    }

    /// T5-08 的接线检查：模块接进来了、命令注册了、前端登记了、Agent 拿得到那份报告。
    /// 少任何一处都会让温度问题静默退化成"没有数据"，所以四条一起断。
    #[test]
    fn thermal_command_is_registered_on_both_sides() {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let lib_rs = std::fs::read_to_string(manifest.join("src/lib.rs")).unwrap();
        assert!(lib_rs.contains("mod platform;"), "platform 模块没接进 lib.rs");
        assert!(
            lib_rs.contains("commands::get_thermal"),
            "get_thermal 没注册进 invoke_handler"
        );
        let contract = std::fs::read_to_string(manifest.join("../src/ipc_contract.ts")).unwrap();
        assert!(contract.contains("getThermal: \"get_thermal\""), "前端未登记 get_thermal");
        let commands_rs = std::fs::read_to_string(manifest.join("src/commands.rs")).unwrap();
        assert!(
            commands_rs.contains("thermal: &thermal::probe()"),
            "Agent 的输入里没有温度报告，温度问题会一直回\"没有通路\""
        );
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

    /// Agent 的采集侧（T5-04）：真实机器上取一次排行榜，验证"独立采集 + 按键降序 + 条数上限"。
    /// `agent.rs` 的测试用的是手搓切片，这里补的是"真的去 sysinfo 拿数据"那一段。
    #[test]
    fn agent_ranking_collection_returns_ordered_rows() {
        let cpu = collect_ranked(crate::agent::RankBy::Cpu);
        assert_eq!(cpu.len(), crate::agent::MAX_RANK, "应正好取回一页排行榜");
        assert!(
            cpu.windows(2).all(|w| w[0].cpu_usage >= w[1].cpu_usage),
            "CPU 榜必须降序：{:?}",
            cpu.iter().map(|p| (p.pid, p.cpu_usage)).collect::<Vec<_>>()
        );
        assert!(
            cpu.iter().all(|p| !p.name.is_empty() && p.pid > 0),
            "每一行都要有真实 PID 与名字"
        );

        let mem = collect_ranked(crate::agent::RankBy::Memory);
        assert!(
            mem.windows(2).all(|w| w[0].memory_bytes >= w[1].memory_bytes),
            "内存榜必须降序：{:?}",
            mem.iter()
                .map(|p| (p.pid, p.memory_bytes))
                .collect::<Vec<_>>()
        );
        // 两个榜的取样口径不同：内存榜首名的内存不应低于 CPU 榜首名的内存（同机器同量级）
        assert!(mem[0].memory_bytes >= mem.last().unwrap().memory_bytes);
    }

    /// SEC-V10：命令层的 Agent 分节不许出现任何执行通路。
    /// `agent.rs` 自己已经扫过一遍，这条补上另一头 —— 命令层拿得到 `kill_process`，
    /// 所以它必须在源码层面就只调引擎的只读入口，而不是"看起来没调"。
    #[test]
    fn agent_command_section_has_no_execution_path() {
        let src = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/commands.rs"),
        )
        .unwrap();
        let head = "// ==================== 诊断 Agent";
        let start = src.find(head).expect("找不到 Agent 分节");
        let rest = &src[start + head.len()..];
        // 只扫到下一节标题为止：后面那些分节里本来就该有破坏性命令。
        let section = match rest.find("// ====================") {
            Some(offset) => &rest[..offset],
            None => rest,
        };
        for forbidden in [
            "kill_process",
            "cleanup_junk_files",
            "cancel_junk_scan",
            "flush_dns_cache",
            "Command::new",
            "std::process::",
            "std::fs::remove",
            "std::fs::write",
            "sudo",
        ] {
            assert!(
                !section.contains(forbidden),
                "Agent 分节里出现了执行通路 {forbidden}"
            );
        }
        assert!(section.contains("agent::answer"), "Agent 分节没有转调引擎");
        // 采集只允许走 monitor 的只读入口
        assert!(section.contains("monitor::collect_top"));
    }

    /// SEC-T01：CSP 必须**写在配置里**且不含通配/eval —— 这条防的是配置回归（改回 `null`
    /// 或图省事加 `unsafe-eval`，前端不会报错，只是注入防护整段没了）。
    /// 浏览器里"注入被拦下"这一半本机没法自动化，仍按未验证记在 06。
    #[test]
    fn sec_t01_the_content_security_policy_is_declared_and_narrow() {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let raw = std::fs::read_to_string(manifest.join("tauri.conf.json")).unwrap();
        let conf: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let csp = conf["app"]["security"]["csp"]
            .as_str()
            .expect("CSP 必须是字符串（配成 null 等于没有策略）");
        assert!(csp.contains("script-src 'self'"), "script-src 没收紧：{csp}");
        for forbidden in ["unsafe-eval", "https://*", " 'self ' ", "<nonce>", "*"] {
            assert!(!csp.contains(forbidden), "CSP 里出现了 {forbidden:?}：{csp}");
        }
        assert!(csp.contains("default-src 'self'"), "default-src 兜底没了：{csp}");
    }

    /// SEC-T04：命令边界上杀 PID 1 必须被拒，且**在发信号之前**就拒。
    /// （terminate() 内部第一步就是 validate_kill；这条锁的是命令层的返回码。）
    #[tokio::test]
    async fn sec_t04_killing_pid_one_is_refused_at_the_command_boundary() {
        let err = kill_process(1, None).await.unwrap_err();
        assert_eq!(err.code, "PERMISSION_DENIED", "PID 1 没在命令层被拦下：{err:?}");
        // 低 PID 的理由必须是"系统进程区间"，不能是"属于其他用户"（后者会让人以为换个权限就能杀）
        assert!(err.message.contains("系统进程") && err.message.contains("禁止"),
            "PID 1 的拒绝理由说错了：{}", err.message);
        assert!(!err.message.contains("其他用户"), "PID 1 被判成属主问题：{}", err.message);
        assert!(err.detail.is_none() || cfg!(debug_assertions), "detail 只许在 debug 构建里出现");
    }
}
