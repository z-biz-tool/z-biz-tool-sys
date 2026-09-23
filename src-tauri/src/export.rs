//! 历史趋势导出 CSV —— 02 文档 F5 目标里的"可导出 CSV"（审计外补做，2026-09-23）。
//!
//! 落盘边界**不另起一套规则**：路径校验走 `prefs::resolve_target_path`（同一个函数，只是后缀与
//! 体积上限不同），原子写走 `prefs::write_bytes_atomically`。两处各写一份校验迟早漂移，
//! 漂成"CSV 能写到受保护目录而偏好 JSON 不能"这种只有挨个测才能发现的问题。
//!
//! 数据真实性上有两条刻意的取舍：
//! 1. **不静默截断。** 行数/体积超上限时直接拒，并让用户缩小时间范围 —— 一份少了尾巴的 CSV
//!    看起来跟完整的一样，那是最坏的一种错。
//! 2. **每一行都带着它的桶宽。** 窗口大时 `history::query` 会做分桶均值压缩，此时
//!    `cpu_percent` 是"这一分钟的平均"而不是某一帧的读数。不写 `bucket_seconds` 的话，
//!    拿 CSV 去二次计算的人会把这个当成瞬时值。

use crate::error::{AppError, CommandResult};
use crate::history::{HistoryPage, HistoryStore, MAX_SPAN_SECS};
use crate::prefs::{resolve_target_path, write_bytes_atomically};
use serde::Serialize;

/// 表头。列全是数值，不含路径、挂载点、进程名 —— 导出文件会被用户随手发到别处，
/// 少一类字段就少一类泄漏面，而且这些列对"看趋势"本来也没用。
pub const CSV_HEADER: &str = "timestamp_ms,cpu_percent,memory_percent,rx_bytes_per_sec,tx_bytes_per_sec,bucket_seconds\n";
/// 写之前的体积上限。当前保留窗口（7 天 + 分桶压缩）导出的量级约 1 万行 / 数百 KB，
/// 这道闸平时不会碰到 —— 它是防止以后窗口或分辨率变大时**静默写出一个巨大文件**的兜底。
pub const MAX_CSV_BYTES: u64 = 16 * 1024 * 1024;

/// 导出结果。字段名与前端 `CsvExportOutcome` 逐一对齐，有契约测试锁定。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CsvExportOutcome {
    /// 实际写入的路径（`~` 已展开、符号链接已解析）
    pub path: String,
    pub bytes: u64,
    /// 真实写出去的数据行数（不含表头）
    pub rows: usize,
    pub span_seconds: u64,
    /// 导出内容的分辨率：等于 10 时是原始采样点，更大时是桶均值
    pub bucket_seconds: u64,
    /// 该区间内实际落盘的点数（压缩之前）。与 `rows` 一起才能说明"有没有被压缩过"
    pub stored_points: usize,
    pub oldest_ms: Option<u64>,
    pub newest_ms: Option<u64>,
    /// 读盘时解析失败的行数：如实带出来，不在导出里修补
    pub unreadable_lines: usize,
}

/// 把一页历史渲染成 CSV 文本。纯函数，所以"数值精度、桶宽列、空历史只剩表头"这些都能直接测。
pub fn build_csv(page: &HistoryPage) -> String {
    let mut out = String::from(CSV_HEADER);
    for point in &page.points {
        out.push_str(&format!(
            "{},{:.2},{:.2},{:.0},{:.0},{}\n",
            point.t, point.cpu, point.memory, point.rx_bytes_per_sec, point.tx_bytes_per_sec, page.bucket_seconds
        ));
    }
    out
}

/// 导出到用户选定的路径。`now_ms` 由调用方给（测试要能钉住时间窗口）。
pub fn write_history_csv(store: &HistoryStore, path: &str, now_ms: u64, span_secs: u64) -> CommandResult<CsvExportOutcome> {
    write_history_csv_with_limit(store, path, now_ms, span_secs, MAX_CSV_BYTES)
}

/// `max_bytes` 单独开一个参数，是为了让"超限就拒、不静默截断"这条分支能被真的端到端测到：
/// 用默认上限的话在当前保留窗口下永远走不到那里，只测纯函数等于没测调用点。
fn write_history_csv_with_limit(
    store: &HistoryStore,
    path: &str,
    now_ms: u64,
    span_secs: u64,
    max_bytes: u64,
) -> CommandResult<CsvExportOutcome> {
    let page = crate::history::query(store, now_ms, span_secs);
    let target = resolve_target_path(path, false, "csv", "历史 CSV", max_bytes)?;
    let text = build_csv(&page);
    if text.len() as u64 > max_bytes {
        return Err(AppError::invalid_input(format!(
            "导出内容 {} B 超过 {} B 上限（{} 行、保留窗口 {} 天），请缩小时间范围后再导出",
            text.len(),
            max_bytes,
            page.points.len(),
            MAX_SPAN_SECS / 86_400
        )));
    }

    write_bytes_atomically(&target, text.as_bytes())?;

    Ok(CsvExportOutcome {
        // 回给用户看的是 canonical 后的真身：与偏好导出同一口径，避免"写到了别处"
        path: target.to_string_lossy().to_string(),
        bytes: text.len() as u64,
        rows: page.points.len(),
        span_seconds: page.span_seconds,
        bucket_seconds: page.bucket_seconds,
        stored_points: page.stored_points,
        oldest_ms: page.oldest_ms,
        newest_ms: page.newest_ms,
        unreadable_lines: page.unreadable_lines,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::{HistoryPoint, HistoryStore, SAMPLE_INTERVAL_MS};
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    fn fixture(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "zsys-csv-{}-{}-{}",
            std::process::id(),
            tag,
            SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("临时目录应可建");
        dir
    }

    fn point(t: u64, cpu: f64, memory: f64) -> HistoryPoint {
        HistoryPoint {
            t,
            cpu,
            memory,
            rx_bytes_per_sec: cpu * 8.0,
            tx_bytes_per_sec: memory * 2.0,
        }
    }

    fn page(points: Vec<HistoryPoint>, bucket: u64) -> HistoryPage {
        let stored_points = points.len();
        HistoryPage {
            points,
            span_seconds: 3_600,
            bucket_seconds: bucket,
            stored_points,
            oldest_ms: None,
            newest_ms: None,
            unreadable_lines: 0,
        }
    }

    /// 表头 + 每行的六个字段，数值精度固定，且每行都带桶宽。
    #[test]
    fn the_csv_is_a_header_plus_one_line_per_point_with_its_bucket_width() {
        let text = build_csv(&page(
            vec![point(1_700_000_000_000, 12.5, 60.25), point(1_700_000_010_000, 100.0, 0.0)],
            10,
        ));
        let lines: Vec<&str> = text.trim_end().split('\n').collect();
        assert_eq!(lines[0], CSV_HEADER.trim_end());
        assert_eq!(lines.len(), 3, "两行数据 + 一行表头：{lines:?}");
        assert_eq!(
            lines[1],
            "1700000000000,12.50,60.25,100,120,10",
            "数值精度或列序变了：{}",
            lines[1]
        );
        assert_eq!(lines[2], "1700000010000,100.00,0.00,800,0,10");
        // 每一行都自带桶宽，二次计算的人不会把分钟均值当成瞬时值
        assert!(lines[1..].iter().all(|l| l.ends_with(",10")));
    }

    /// 压缩过的窗口要能在每行上看出桶宽（这里是 60 s 均值）。
    #[test]
    fn bucket_averaged_rows_say_so_in_every_line() {
        let text = build_csv(&page(vec![point(1, 50.4, 50.6)], 60));
        assert!(text.lines().nth(1).expect("应有一行数据").ends_with(",60"));
    }

    /// 空历史：只有表头，且不报错 —— "这阵子没数据"是一个真实状态，界面据此说"0 行"。
    #[test]
    fn an_empty_window_exports_a_header_only_file_instead_of_faking_rows() {
        let text = build_csv(&page(Vec::new(), 10));
        assert_eq!(text, CSV_HEADER);
        assert_eq!(text.lines().count(), 1);
    }

    /// 导出文件里不许出现路径类内容（列就六个，全是数值）。
    #[test]
    fn the_exported_file_carries_no_paths_or_names() {
        let text = build_csv(&page(vec![point(1_700_000_000_000, 1.0, 2.0)], 10));
        for forbidden in ["/Users", "/Volumes", "\\", "mount", "name"] {
            assert!(!text.contains(forbidden), "导出内容里出现了 {forbidden:?}：{text}");
        }
    }

    /// 超限必须**拒**，不能悄悄截断：一份少了尾巴的 CSV 看起来和完整的一样。
    #[test]
    fn refusing_an_oversized_export_leaves_no_file_and_says_narrow_the_window() {
        let store_dir = fixture("rows");
        let store = HistoryStore::new(&store_dir);
        let now = 1_700_000_000_000;
        for i in 0..12u64 {
            store.append(point(now - i * SAMPLE_INTERVAL_MS, 33.3, 44.4)).unwrap();
        }
        let target = store_dir.join("out.csv");
        let err = write_history_csv_with_limit(&store, &target.to_string_lossy(), now, 3_600, 64)
            .unwrap_err();
        assert_eq!(err.code, "INVALID_INPUT", "超限必须拒而不是截断：{err:?}");
        assert!(err.message.contains("缩小"), "得告诉用户怎么补救：{}", err.message);
        assert!(err.message.contains("12 行"), "要报出真实行数而不是含糊带过：{}", err.message);
        assert!(!target.exists(), "拒掉之后不该留下半个文件");
        let leftovers: Vec<String> = fs::read_dir(&store_dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|name| name.contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty(), "拒掉时也不该留下临时文件：{leftovers:?}");

        // 同一份数据放在够用的上限下要真的写出来（否则上面的拒可能是别的原因）
        let ok = write_history_csv_with_limit(&store, &target.to_string_lossy(), now, 3_600, MAX_CSV_BYTES).unwrap();
        assert_eq!(ok.rows, 12, "{ok:?}");
        assert!(target.exists());
        let _ = fs::remove_dir_all(&store_dir);
    }

    /// 落盘边界与偏好导出同一套：非 .csv、目录、受保护位置都要被拒。
    #[test]
    fn the_same_path_rules_as_the_prefs_export_apply_here() {
        let dir = fixture("guards");
        let store = HistoryStore::new(&dir);
        store.append(point(1_700_000_000_000, 1.0, 1.0)).unwrap();

        let wrong_suffix = dir.join("out.txt");
        let err = write_history_csv(&store, &wrong_suffix.to_string_lossy(), 1_700_000_000_000 + 10, 3_600)
            .unwrap_err();
        assert_eq!(err.code, "INVALID_INPUT");
        assert!(err.message.contains(".csv"), "错误要指明后缀要求：{}", err.message);

        let err = write_history_csv(&store, &dir.to_string_lossy(), 1_700_000_000_000 + 10, 3_600).unwrap_err();
        assert_eq!(err.code, "INVALID_INPUT", "指向目录必须被拒：{err:?}");

        // 受保护位置分两种：本机真有的目录，和"这个平台上根本不存在"的目录。
        // 第二种正是 Linux 腿红过的形状 —— /System/Volumes/Data 只有 macOS 有，守卫先去
        // canonicalize 父目录，就把 PATH_DENIED 降级成了 NOT_FOUND，结论取决于文件系统运气。
        let present_dir = if cfg!(windows) {
            "C:\\Windows\\Temp\\history.csv"
        } else {
            "/System/Volumes/Data/history.csv"
        };
        let missing_dir = if cfg!(windows) {
            "C:\\Windows\\Nonexistent-Zsys\\history.csv"
        } else {
            "/usr/local/share/zsys-missing-dir/history.csv"
        };
        for denied in [present_dir, missing_dir] {
            let err = write_history_csv(&store, denied, 1_700_000_000_000 + 10, 3_600).unwrap_err();
            assert_eq!(err.code, "PATH_DENIED", "受保护位置不该被写（{denied}）：{err:?}");
        }

        let missing_parent = std::env::temp_dir()
            .join(format!("zsys-csv-不存在-{}", std::process::id()))
            .join("history.csv");
        let err = write_history_csv(&store, &missing_parent.to_string_lossy(), 1, 3_600).unwrap_err();
        assert_eq!(err.code, "NOT_FOUND", "所在目录不存在时不该顺手创建：{err:?}");

        let _ = fs::remove_dir_all(&dir);
    }

    /// 真写一遍：内容是 CSV、行数与 `get_history` 同源、且不留临时文件。
    #[test]
    fn a_real_export_lands_on_disk_with_the_same_rows_the_chart_reads() {
        let dir = fixture("write");
        let store = HistoryStore::new(&dir);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(1_700_000_000_000);
        for i in 0..5 {
            store
                .append(point(now - i * SAMPLE_INTERVAL_MS, 20.0 + i as f64, 55.5))
                .unwrap();
        }
        let target = dir.join("history.csv");
        let outcome = write_history_csv(&store, &target.to_string_lossy(), now, 3_600).unwrap();
        assert_eq!(outcome.rows, 5, "{outcome:?}");
        assert_eq!(outcome.span_seconds, 3_600);
        assert_eq!(outcome.bucket_seconds, SAMPLE_INTERVAL_MS / 1000);
        assert_eq!(outcome.rows, outcome.stored_points, "小窗口不该被压缩");
        assert_eq!(outcome.unreadable_lines, 0);
        assert!(outcome.oldest_ms.is_some() && outcome.newest_ms.is_some());

        let text = fs::read_to_string(&target).unwrap();
        assert!(text.starts_with(CSV_HEADER));
        assert_eq!(text.trim_end().lines().count(), outcome.rows + 1);
        assert_eq!(outcome.bytes, text.len() as u64);
        // 与前端读的是同一份查询：行数、桶宽都要一致
        let chart = crate::history::query(&store, now, 3_600);
        assert_eq!(chart.points.len(), outcome.rows);
        assert_eq!(chart.bucket_seconds, outcome.bucket_seconds);

        // 再导出一次应当整体覆盖，不留 .tmp 半成品
        let again = write_history_csv(&store, &target.to_string_lossy(), now, 3_600).unwrap();
        assert_eq!(again.rows, 5);
        let leftovers: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|name| name.contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty(), "留下了临时文件：{leftovers:?}");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 2, "目录里应只有历史文件与导出的 CSV");

        let _ = fs::remove_dir_all(&dir);
    }

    /// 载荷字段名必须与前端 interface 对齐：漂了的后果和 T5-08 一样安静（`undefined` 当 0 显示）。
    #[test]
    fn the_export_outcome_payload_matches_the_frontend_interface() {
        use crate::contract_fixtures::{serialized_keys, ts_interface_keys};

        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let contract = std::fs::read_to_string(manifest.join("../src/ipc_contract.ts")).unwrap();
        let outcome = CsvExportOutcome {
            path: String::new(),
            bytes: 0,
            rows: 0,
            span_seconds: 0,
            bucket_seconds: 0,
            stored_points: 0,
            oldest_ms: None,
            newest_ms: None,
            unreadable_lines: 0,
        };
        assert_eq!(
            serialized_keys(&serde_json::to_value(&outcome).unwrap()),
            ts_interface_keys(&contract, "CsvExportOutcome"),
            "CsvExportOutcome 的字段名与前端不一致"
        );
        assert!(
            contract.contains("exportHistoryCsv: \"export_history_csv\""),
            "前端未登记 export_history_csv"
        );
    }

    /// 接线：命令注册了、模块进来了，且前端那句"导出 CSV"确实打到这条命令上。
    #[test]
    fn the_export_is_wired_on_both_sides() {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let lib_rs = std::fs::read_to_string(manifest.join("src/lib.rs")).unwrap();
        assert!(lib_rs.contains("mod export;"), "export 模块没接进 lib.rs");
        assert!(
            lib_rs.contains("commands::export_history_csv"),
            "export_history_csv 没注册进 invoke_handler"
        );
        let commands_rs = std::fs::read_to_string(manifest.join("src/commands.rs")).unwrap();
        assert!(
            commands_rs.contains("export::write_history_csv"),
            "命令没有转调 export 模块"
        );
        let overview = std::fs::read_to_string(manifest.join("../src/components/tabs/OverviewTab.tsx")).unwrap();
        assert!(
            overview.contains("onExportCsv"),
            "概览上的导出按钮没接回调"
        );
        let app = std::fs::read_to_string(manifest.join("../src/App.tsx")).unwrap();
        assert!(
            app.contains("Commands.exportHistoryCsv"),
            "App 没通过 export_history_csv 命令导出"
        );
    }
}
