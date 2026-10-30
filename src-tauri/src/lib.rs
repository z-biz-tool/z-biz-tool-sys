mod agent;
mod alert;
mod cleanup;
mod commands;
#[cfg(test)]
mod contract_fixtures;
mod error;
mod export;
mod history;
mod log_sanitize;
mod monitor;
mod netinfo;
mod notify;
mod observe;
mod platform;
mod prefs;
mod safety;
mod startup;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let service = monitor::MonitorService::new();
    service.publish();
    let alert_config = alert::AlertState::default();
    // 系统通知（T5-02）：投递结果的记账。循环里写、`notify_status` 里读，两边各持一份同一份计数。
    let notify = notify::NotifyState::default();
    // 深链队列：manage 给命令用、seed 给冷启动用，两处必须是同一份。
    let zbiz_pending = cap_hub::receiver::SharedPending::default();

    tauri::Builder::default()
        // 单实例（2026-10-26 分发底座）：第二次启动聚焦已有窗口，不再开新进程。
        // 必须作为第一个注册的插件（tauri-plugin-single-instance 官方要求）。
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
            cap_hub::receiver::handle_second_instance(app, &argv);
        }))
        .plugin(tauri_plugin_dialog::init())
        // 通知只从后端 Rust API 投（`notify` 模块头写了为什么不走 JS 通道），
        // 所以 capabilities 不需要加任何 notification 权限项。
        .plugin(tauri_plugin_notification::init())
        // 自动更新（2026-10-25 接入，方案与 z-biz-tool-file 一致）：启动后由前端
        // UpdaterDialog 延时调用 check()，endpoint 与 pubkey 见 tauri.conf.json。
        // pubkey 目前是占位符，CEO 填入真实公钥前 check() 会失败并按设计静默降级。
        .plugin(tauri_plugin_updater::Builder::new().build())
        // 更新安装完成后的「立即重启」
        .plugin(tauri_plugin_process::init())
        .manage(service.clone())
        .manage(alert_config.clone())
        .manage(notify.clone())
        .manage(zbiz_pending.clone())
        .setup(move |app| {
            // 冷启动时带进来的 zbiz:// 先入队，前端挂上监听后再取走。
            cap_hub::receiver::seed_cold_start(app.handle(), &zbiz_pending);
            // 历史存储只在启动时解析一次：采集循环拿写入端，command 拿只读端。
            let store = history::store_for(app.handle());
            app.manage(history::HistoryState(store.clone()));
            // 告警历史同理（T5-03）：循环里写、`get_alert_history` 里读，两个各持一份路径相同的句柄。
            let alert_store = history::alert_store_for(app.handle());
            app.manage(history::AlertHistoryState(alert_store.clone()));
            service.spawn(
                app.handle().clone(),
                store,
                alert_store,
                alert_config.0.clone(),
                notify.clone(),
            );
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_static_info,
            commands::get_metrics_snapshot,
            commands::get_history,
            commands::get_alert_history,
            commands::set_monitor_config,
            commands::get_alert_config,
            commands::set_alert_config,
            commands::start_process_stream,
            commands::stop_process_stream,
            commands::set_process_query,
            commands::get_processes,
            commands::get_process_rollup,
            commands::get_listening_sockets,
            commands::get_process_detail,
            commands::validate_kill,
            commands::kill_process,
            commands::flush_dns_cache,
            commands::scan_junk_files,
            commands::cancel_junk_scan,
            commands::cleanup_junk_files,
            commands::find_large_files_cmd,
            commands::get_startup_items_cmd,
            commands::disable_startup_item,
            commands::remove_startup_item,
            commands::restore_startup_item,
            commands::export_prefs_file,
            commands::import_prefs_file,
            commands::agent_query,
            commands::get_thermal,
            commands::export_history_csv,
            commands::notify_status,
            commands::send_test_notification,
            zbiz_take_pending,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// 取走队列里的深链（家族统一命令名）。
#[tauri::command]
fn zbiz_take_pending(
    pending: tauri::State<'_, cap_hub::receiver::SharedPending>,
) -> Vec<cap_hub::Handoff> {
    cap_hub::receiver::take_pending_from(pending.inner())
}
