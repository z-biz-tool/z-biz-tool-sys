mod alert;
mod cleanup;
mod commands;
mod error;
mod history;
mod log_sanitize;
mod monitor;
mod netinfo;
mod prefs;
mod safety;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let service = monitor::MonitorService::new();
    service.publish();
    let alert_config = alert::AlertState::default();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(service.clone())
        .manage(alert_config.clone())
        .setup(move |app| {
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
            commands::get_process_detail,
            commands::validate_kill,
            commands::kill_process,
            commands::flush_dns_cache,
            commands::scan_junk_files,
            commands::cancel_junk_scan,
            commands::cleanup_junk_files,
            commands::find_large_files_cmd,
            commands::get_startup_items_cmd,
            commands::export_prefs_file,
            commands::import_prefs_file,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
