mod cleanup;
mod commands;
mod error;
mod log_sanitize;
mod monitor;
mod netinfo;
mod safety;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let service = monitor::MonitorService::new();
    service.publish();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(service.clone())
        .setup(move |app| {
            service.spawn(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_static_info,
            commands::get_metrics_snapshot,
            commands::set_monitor_config,
            commands::start_process_stream,
            commands::stop_process_stream,
            commands::get_processes,
            commands::validate_kill,
            commands::kill_process,
            commands::flush_dns_cache,
            commands::scan_junk_files,
            commands::cleanup_junk_files,
            commands::find_large_files_cmd,
            commands::get_startup_items_cmd,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
