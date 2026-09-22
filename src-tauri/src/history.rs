//! 趋势历史落盘（T3-07）。
//!
//! 为什么要有这个模块：实时曲线只存在前端内存里，重启后 1 小时窗口要重新攒一小时数据，
//! "看刚才那波占用去哪了"这个问题永远答不上来。这里每 10 s 落一个点、保留 7 天。
//!
//! 刻意不引入 SQLite/chrono：单文件 JSON Lines 追加 + 按保留期重写已经够用，
//! 而依赖表每多一项，release 体积与 A-08 常驻内存目标都更难看。

use crate::log_sanitize::sanitize;
use crate::monitor::{epoch_ms, MetricsSnapshot};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

/// 目录/文件名由后端固定生成，不接受任何外部输入，避免路径注入面。
pub const HISTORY_DIR_NAME: &str = "history";
pub const HISTORY_FILE_NAME: &str = "points.jsonl";

/// 落盘采样间隔：`sys://metrics` 最快 1 s 一帧，按此节流。
pub const SAMPLE_INTERVAL_MS: u64 = 10_000;
/// 保留窗口：7 天。
pub const RETENTION_MS: u64 = 7 * 24 * 3600 * 1000;
/// 查询跨度上下界；超过上界按上界夹取，不做"多读一点"的含糊处理。
pub const MIN_SPAN_SECS: u64 = 60;
pub const MAX_SPAN_SECS: u64 = 7 * 24 * 3600;
/// 前端未指定跨度时的默认值：与趋势图的 1 小时窗口对齐。
pub const DEFAULT_SPAN_SECS: u64 = 3600;
/// 单次响应最多返回的点数。7 天全量约 6 万点，直接过 IPC 既拖慢前端也画不动。
pub const MAX_PAGE_POINTS: usize = 600;

/// 一个采样点。字段名与前端 `HistoryPoint` 逐一对齐，有跨语言契约测试锁定。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPoint {
    /// 毫秒时间戳，与 `MetricsSnapshot.timestampMs` 同源
    pub t: u64,
    pub cpu: f64,
    /// 内存使用率百分比（与趋势图口径一致，不是字节）
    pub memory: f64,
    pub rx_bytes_per_sec: f64,
    pub tx_bytes_per_sec: f64,
}

impl HistoryPoint {
    /// 网络速率与前端实时曲线同口径：所有网卡求和。
    pub fn from_snapshot(snapshot: &MetricsSnapshot) -> Self {
        Self {
            t: snapshot.timestamp_ms,
            cpu: snapshot.cpu.total,
            memory: snapshot.memory.usage_percent,
            rx_bytes_per_sec: snapshot
                .networks
                .iter()
                .map(|n| n.rx_bytes_per_sec)
                .sum(),
            tx_bytes_per_sec: snapshot
                .networks
                .iter()
                .map(|n| n.tx_bytes_per_sec)
                .sum(),
        }
    }
}

/// `get_history` 的返回：点 + 这批点的真实分辨率与覆盖情况。
///
/// `bucketSeconds`/`storedPoints`/`oldestMs` 存在的理由：调用方必须能分辨
/// "这 1 小时真有 360 个 10 s 点"和"只有 20 分钟数据、按桶平均出来的 120 个点"，
/// 否则压缩过的曲线看起来就像完整历史。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPage {
    pub points: Vec<HistoryPoint>,
    pub span_seconds: u64,
    pub bucket_seconds: u64,
    /// 该区间内实际落盘的点数（分桶压缩之前）
    pub stored_points: usize,
    /// 文件里最早可回溯到的点；无历史时为 `None`，不补任何合成点
    pub oldest_ms: Option<u64>,
    pub newest_ms: Option<u64>,
    /// 解析失败的行数。进程被强杀时尾部留半行是正常后果，只报告不修补
    pub unreadable_lines: usize,
}

#[derive(Debug, Clone)]
pub struct HistoryStore {
    path: PathBuf,
}

/// 一次全量读取的结果：点按时间递增（文件本身按追加顺序写入），外加坏行计数。
#[derive(Debug, Default)]
pub struct ReadAll {
    pub points: Vec<HistoryPoint>,
    pub unreadable_lines: usize,
    /// `None` 表示文件不存在（首次运行），这不算失败
    pub io_error: Option<String>,
}

impl HistoryStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            path: dir.into().join(HISTORY_FILE_NAME),
        }
    }

    fn ensure_parent(&self) -> std::io::Result<()> {
        let dir = self
            .path
            .parent()
            .ok_or_else(|| std::io::Error::other("历史路径没有父目录"))?;
        fs::create_dir_all(dir)
    }

    /// 追加一行。每次单独开句柄：写入频率是 10 s 一次，省下的开销远不如
    /// "修剪要重写文件、长持句柄会写到已改名的旧 inode"带来的麻烦。
    pub fn append(&self, point: HistoryPoint) -> std::io::Result<()> {
        self.ensure_parent()?;
        let mut file = OpenOptions::new().create(true).append(true).open(&self.path)?;
        let line = serde_json::to_string(&point)
            .map_err(|e| std::io::Error::other(format!("序列化失败: {e}")))?;
        writeln!(file, "{line}")
    }

    /// 逐行解析。坏行只跳过并计数，绝不补零 —— 半行是崩溃留下的，不是数据。
    pub fn read_all(&self) -> ReadAll {
        let mut out = ReadAll::default();
        let file = match fs::File::open(&self.path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return out,
            Err(e) => {
                out.io_error = Some(sanitize(&e.to_string()));
                return out;
            }
        };
        for line in BufReader::new(file).lines() {
            let Ok(line) = line else { break };
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<HistoryPoint>(&line) {
                Ok(point) => out.points.push(point),
                Err(_) => out.unreadable_lines += 1,
            }
        }
        out
    }

    /// 重写整个文件（修剪用）。先写 `.tmp` 再 rename，避免读到半截文件。
    pub fn rewrite(&self, points: &[HistoryPoint]) -> std::io::Result<()> {
        self.ensure_parent()?;
        let tmp = self.path.with_extension("jsonl.tmp");
        {
            let mut file = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&tmp)?;
            for point in points {
                let line = serde_json::to_string(point)
                    .map_err(|e| std::io::Error::other(format!("序列化失败: {e}")))?;
                writeln!(file, "{line}")?;
            }
            file.flush()?;
        }
        fs::rename(&tmp, &self.path)
    }
}

/// 采集循环里的写入器：负责 10 s 节流、时间戳单调、过期修剪。
///
/// 只由 `run_metrics_loop` 的单一任务持有 `&mut`，因此不需要锁；读取侧走
/// [`HistoryStore`] 的只读方法，靠 rename 的原子性拿到要么旧要么新的完整文件。
pub struct HistoryRecorder {
    store: HistoryStore,
    last_written_ms: Option<u64>,
    oldest_ms: Option<u64>,
    pub write_failures: u64,
    pub dropped_stale_points: u64,
    pub pruned_points: u64,
}

impl HistoryRecorder {
    /// 打开并把已有文件的时间戳基准读回来：重启后若不读，第一帧就会比文件尾部更早，
    /// 破坏"按时间递增"这一修剪与分桶共同依赖的前提。
    pub fn open(store: HistoryStore) -> Self {
        let read = store.read_all();
        Self {
            store,
            last_written_ms: read.points.last().map(|p| p.t),
            oldest_ms: read.points.first().map(|p| p.t),
            write_failures: 0,
            dropped_stale_points: 0,
            pruned_points: 0,
        }
    }

    /// 收到一个候选点，返回是否真的落盘。
    pub fn observe(&mut self, point: HistoryPoint) -> bool {
        if let Some(last) = self.last_written_ms {
            if point.t <= last {
                // 系统时钟被往回调（改时间、唤醒后校正）时，写进去会让文件不再单调递增，
                // 修剪与分桶都会算错。丢掉这一帧比写坏文件诚实：图上是一个缺口，不是假数据。
                self.dropped_stale_points += 1;
                return false;
            }
            if point.t - last < SAMPLE_INTERVAL_MS {
                return false;
            }
        }

        match self.store.append(point) {
            Ok(()) => {
                self.last_written_ms = Some(point.t);
                self.oldest_ms.get_or_insert(point.t);
                self.pruned_points += self.prune_if_expired(point.t) as u64;
                true
            }
            Err(e) => {
                self.write_failures += 1;
                if self.write_failures == 1 {
                    eprintln!("[history] 写入失败: {}", sanitize(&e.to_string()));
                }
                false
            }
        }
    }

    /// 只在"确实有数据过期"时重写文件，所以正常运行的 7 天里大约每天一次。
    fn prune_if_expired(&mut self, now_ms: u64) -> usize {
        let Some(oldest) = self.oldest_ms else {
            return 0;
        };
        if now_ms.saturating_sub(oldest) < RETENTION_MS {
            return 0;
        }
        let read = self.store.read_all();
        let total = read.points.len();
        let kept: Vec<HistoryPoint> = read
            .points
            .into_iter()
            .filter(|p| now_ms.saturating_sub(p.t) < RETENTION_MS)
            .collect();
        let dropped = total - kept.len();
        match self.store.rewrite(&kept) {
            Ok(()) => {
                self.oldest_ms = kept.first().map(|p| p.t);
                self.last_written_ms = kept.last().map(|p| p.t).or(self.last_written_ms);
                dropped
            }
            Err(e) => {
                eprintln!("[history] 修剪失败: {}", sanitize(&e.to_string()));
                0
            }
        }
    }
}

/// 查询 `[now_ms - span, now_ms]` 区间的历史，超过点数上限时按桶取均值。
///
/// 分桶是真实的聚合（不是插值）：`bucketSeconds` 会告诉调用方每个点代表多长区段的平均。
pub fn query(store: &HistoryStore, now_ms: u64, span_secs: u64) -> HistoryPage {
    let span_secs = span_secs.clamp(MIN_SPAN_SECS, MAX_SPAN_SECS);
    let read = store.read_all();
    let from = now_ms.saturating_sub(span_secs * 1000);
    let matched: Vec<HistoryPoint> = read
        .points
        .iter()
        .filter(|p| p.t >= from)
        .copied()
        .collect();

    let raw_bucket_secs = SAMPLE_INTERVAL_MS / 1000;
    // 只有真的超过点数上限才压缩；否则即便跨度很大，也保持原始 10 s 分辨率，
    // 免得把"7 天里只有 60 个点"这种稀疏数据再平均成 1 个点、反而丢掉信息。
    let bucket_secs = if matched.len() > MAX_PAGE_POINTS {
        ((span_secs as usize).div_ceil(MAX_PAGE_POINTS) as u64).max(raw_bucket_secs)
    } else {
        raw_bucket_secs
    };
    let bucket_ms = bucket_secs * 1000;

    let points = if matched.len() > MAX_PAGE_POINTS {
        average_into_buckets(&matched, from, bucket_ms)
    } else {
        matched.clone()
    };

    HistoryPage {
        points,
        span_seconds: span_secs,
        bucket_seconds: bucket_secs,
        stored_points: matched.len(),
        oldest_ms: read.points.first().map(|p| p.t),
        newest_ms: read.points.last().map(|p| p.t),
        unreadable_lines: read.unreadable_lines,
    }
}

/// 按固定宽度分段取均值，时间戳取该段最后一帧 —— 与前端的 `downsampleHistory` 同一约定。
fn average_into_buckets(points: &[HistoryPoint], from_ms: u64, bucket_ms: u64) -> Vec<HistoryPoint> {
    let mut out: Vec<HistoryPoint> = Vec::new();
    let mut bucket_index = u64::MAX;
    let mut group: Vec<HistoryPoint> = Vec::new();
    for point in points {
        let index = point.t.saturating_sub(from_ms) / bucket_ms;
        if index != bucket_index {
            if let Some(avg) = average_bucket(&group) {
                out.push(avg);
            }
            bucket_index = index;
            group.clear();
        }
        group.push(*point);
    }
    if let Some(avg) = average_bucket(&group) {
        out.push(avg);
    }
    out
}

fn average_bucket(group: &[HistoryPoint]) -> Option<HistoryPoint> {
    let last = group.last()?;
    let n = group.len() as f64;
    let sum = |pick: fn(&HistoryPoint) -> f64| group.iter().map(pick).sum::<f64>();
    Some(HistoryPoint {
        t: last.t,
        cpu: sum(|p| p.cpu) / n,
        memory: sum(|p| p.memory) / n,
        rx_bytes_per_sec: sum(|p| p.rx_bytes_per_sec) / n,
        tx_bytes_per_sec: sum(|p| p.tx_bytes_per_sec) / n,
    })
}

/// managed state：`None` 表示拿不到应用数据目录（受限环境/磁盘故障）。
/// 此时历史不持久化，实时曲线照常工作，读取侧返回空页而不是造数据。
#[derive(Debug, Clone, Default)]
pub struct HistoryState(pub Option<HistoryStore>);

/// 解析应用数据目录并建好子目录；失败只打印一次脱敏日志并返回 `None`。
pub fn store_for(app: &tauri::AppHandle) -> Option<HistoryStore> {
    use tauri::Manager;
    let dir = match app.path().app_data_dir() {
        Ok(dir) => dir.join(HISTORY_DIR_NAME),
        Err(e) => {
            eprintln!("[history] 无法解析应用数据目录: {}", sanitize(&e.to_string()));
            return None;
        }
    };
    if let Err(e) = fs::create_dir_all(&dir) {
        eprintln!("[history] 创建 {} 失败: {}", sanitize(&dir.display().to_string()), sanitize(&e.to_string()));
        return None;
    }
    Some(HistoryStore::new(dir))
}

/// 供 command 使用的入口时间戳（单独抽出来，测试自己注入 `now_ms`）。
pub fn now_ms() -> u64 {
    epoch_ms()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static SEQ: AtomicUsize = AtomicUsize::new(0);

    fn fixture(tag: &str) -> PathBuf {
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "zsys-hist-{}-{}-{}",
            std::process::id(),
            tag,
            n
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn point(t_ms: u64, cpu: f64) -> HistoryPoint {
        HistoryPoint {
            t: t_ms,
            cpu,
            memory: cpu / 2.0,
            rx_bytes_per_sec: cpu * 100.0,
            tx_bytes_per_sec: cpu * 10.0,
        }
    }

    fn cleanup(dir: &Path) {
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn samples_are_written_every_ten_seconds_and_read_back() {
        let dir = fixture("cadence");
        let store = HistoryStore::new(&dir);
        let mut recorder = HistoryRecorder::open(store.clone());
        let base = 1_700_000_000_000;

        assert!(recorder.observe(point(base, 10.0)), "第一个点应当落盘");
        assert!(
            !recorder.observe(point(base + 1_000, 20.0)),
            "距上次不足 10 s 不该写"
        );
        assert!(
            !recorder.observe(point(base + 9_999, 30.0)),
            "差 1 ms 到 10 s 仍不该写"
        );
        assert!(recorder.observe(point(base + 10_000, 40.0)), "整 10 s 应写");
        assert!(recorder.observe(point(base + 25_000, 50.0)), "超过 10 s 应写");

        let read = store.read_all();
        let ts: Vec<u64> = read.points.iter().map(|p| p.t).collect();
        assert_eq!(ts, vec![base, base + 10_000, base + 25_000]);
        assert_eq!(read.unreadable_lines, 0);
        assert_eq!(recorder.write_failures, 0);
        cleanup(&dir);
    }

    #[test]
    fn points_older_than_seven_days_are_pruned_and_the_rest_survive() {
        let dir = fixture("retention");
        let store = HistoryStore::new(&dir);
        let base = 1_700_000_000_000;
        for i in 0..5 {
            store.append(point(base + i * SAMPLE_INTERVAL_MS, i as f64)).unwrap();
        }
        // 未过期时不得重写文件：一次写入触发一次全量重写是纯粹的浪费
        let mut recorder = HistoryRecorder::open(store.clone());
        let expired = base + 4 * SAMPLE_INTERVAL_MS;
        assert_eq!(
            recorder.prune_if_expired(expired),
            0,
            "刚写入即修剪说明判定过松"
        );

        let way_past = base + RETENTION_MS + 1;
        assert!(recorder.observe(point(way_past, 9.0)), "新点应落盘");
        assert!(
            recorder.pruned_points > 0,
            "超过 7 天的点必须被剪掉"
        );

        let read = store.read_all();
        assert!(
            read.points.iter().all(|p| way_past - p.t <= RETENTION_MS),
            "修剪后不得残留过期点"
        );
        assert!(
            read.points.last().map(|p| p.t) == Some(way_past),
            "修剪不能把最新点也丢掉"
        );
        cleanup(&dir);
    }

    #[test]
    fn a_backwards_clock_jump_drops_the_point_instead_of_breaking_order() {
        let dir = fixture("clock");
        let store = HistoryStore::new(&dir);
        let mut recorder = HistoryRecorder::open(store.clone());
        let base = 1_700_000_000_000;
        assert!(recorder.observe(point(base, 10.0)));
        assert!(recorder.observe(point(base + 30_000, 20.0)));
        // 时钟回调 1 小时
        assert!(
            !recorder.observe(point(base + 30_000 - 3_600_000, 30.0)),
            "时间戳倒退的点必须丢弃，否则文件不再单调"
        );
        assert_eq!(recorder.dropped_stale_points, 1);
        assert!(recorder.observe(point(base + 60_000, 40.0)));

        let read = store.read_all();
        let ts: Vec<u64> = read.points.iter().map(|p| p.t).collect();
        assert_eq!(ts, vec![base, base + 30_000, base + 60_000]);
        cleanup(&dir);
    }

    #[test]
    fn unreadable_lines_are_counted_not_repaired() {
        let dir = fixture("broken");
        let store = HistoryStore::new(&dir);
        let base = 1_700_000_000_000;
        store.append(point(base, 10.0)).unwrap();
        {
            // 模拟进程被强杀时只剩半行的尾部
            let mut f = OpenOptions::new()
                .append(true)
                .open(dir.join(HISTORY_FILE_NAME))
                .unwrap();
            f.write_all(b"{\"t\":1700000000010,\"cp").unwrap();
            f.write_all(b"\n").unwrap();
        }
        store.append(point(base + 20_000, 30.0)).unwrap();

        let read = store.read_all();
        assert_eq!(read.points.len(), 2, "坏行前后的完整点仍要读出来");
        assert_eq!(read.unreadable_lines, 1);
        assert_eq!(read.io_error, None);

        let page = query(&store, base + 60_000, MIN_SPAN_SECS);
        assert_eq!(page.unreadable_lines, 1, "坏行数要透到响应里");
        assert_eq!(page.points.len(), 2);
        cleanup(&dir);
    }

    #[test]
    fn a_missing_file_is_an_empty_page_not_zero_filled_series() {
        let dir = fixture("empty");
        let page = query(&HistoryStore::new(dir.join("nope")), 1_700_000_000_000, 3600);
        assert!(page.points.is_empty());
        assert_eq!(page.stored_points, 0);
        assert_eq!(page.oldest_ms, None);
        assert_eq!(page.newest_ms, None);
        assert_eq!(page.unreadable_lines, 0);
        assert_eq!(page.span_seconds, 3600);
        cleanup(&dir);
    }

    #[test]
    fn query_clamps_the_span_and_only_reports_real_coverage() {
        let dir = fixture("clamp");
        let store = HistoryStore::new(&dir);
        let base = 1_700_000_000_000;
        // 只有最近 10 分钟数据
        for i in 0..60 {
            store.append(point(base + i * SAMPLE_INTERVAL_MS, i as f64)).unwrap();
        }
        let now = base + 59 * SAMPLE_INTERVAL_MS;

        let huge = query(&store, now, MAX_SPAN_SECS * 4);
        assert_eq!(huge.span_seconds, MAX_SPAN_SECS, "跨度应夹到保留窗口");

        let tiny = query(&store, now, 1);
        assert_eq!(tiny.span_seconds, MIN_SPAN_SECS, "跨度应夹到下限");

        let hour = query(&store, now, 3600);
        assert_eq!(hour.stored_points, 60);
        assert_eq!(hour.oldest_ms, Some(base), "oldestMs 要能暴露真实起点");
        assert_eq!(hour.bucket_seconds, 10, "点数未超上限时保持原始 10 s 分辨率");
        assert_eq!(hour.points.len(), 60);
        cleanup(&dir);
    }

    #[test]
    fn dense_ranges_are_bucket_averaged_without_inventing_points() {
        let dir = fixture("bucket");
        let store = HistoryStore::new(&dir);
        let base = 1_700_000_000_000;
        // 3000 个 10 s 点 ≈ 8.3 小时真实数据，超过 600 点上限，必须压缩
        const TOTAL: usize = 3000;
        for i in 0..TOTAL {
            store
                .append(point(base + i as u64 * SAMPLE_INTERVAL_MS, (i % 100) as f64))
                .unwrap();
        }
        let now = base + (TOTAL as u64 - 1) * SAMPLE_INTERVAL_MS;
        let dense_span = (TOTAL as u64 * SAMPLE_INTERVAL_MS / 1000) + 600;
        let page = query(&store, now, dense_span);

        assert_eq!(page.stored_points, TOTAL);
        assert!(
            page.points.len() <= MAX_PAGE_POINTS,
            "响应点数应受上限约束，实际 {}",
            page.points.len()
        );
        assert_eq!(
            page.bucket_seconds,
            dense_span.div_ceil(MAX_PAGE_POINTS as u64),
            "压缩后分辨率必须如实上报"
        );
        assert!(
            page.points.len() <= (page.span_seconds / page.bucket_seconds) as usize + 2,
            "点数与桶宽必须自洽：{} 点 / {} s 桶 / {} s 跨度",
            page.points.len(),
            page.bucket_seconds,
            page.span_seconds
        );
        // 桶均值落在原始取值范围内：既不放大，也不凭空造点
        assert!(page.points.iter().all(|p| (0.0..=99.0).contains(&p.cpu)));
        assert!(
            page.points.windows(2).all(|w| w[0].t < w[1].t),
            "分桶后仍须严格递增"
        );
        assert_eq!(
            page.points.last().map(|p| p.t),
            Some(now),
            "最后一个桶要锚在最新点上，不能截尾"
        );

        // 稀疏数据（点数未超上限）即便跨度很大也保持原始 10 s 分辨率，不再平均
        let thin_dir = fixture("bucket-thin");
        let thin = HistoryStore::new(&thin_dir);
        for i in 0..60 {
            thin.append(point(base + i * SAMPLE_INTERVAL_MS, i as f64))
                .unwrap();
        }
        let sparse = query(&thin, now, MAX_SPAN_SECS);
        assert_eq!(sparse.stored_points, 60);
        assert_eq!(sparse.bucket_seconds, SAMPLE_INTERVAL_MS / 1000);
        assert_eq!(sparse.points.len(), 60);
        assert_eq!(sparse.oldest_ms, Some(base), "oldestMs 应是文件里真正的最早点");
        assert!(
            sparse.newest_ms.unwrap() < now,
            "newestMs 应暴露'数据其实没覆盖到 now'，让调用方自己判断缺口"
        );
        cleanup(&thin_dir);
        cleanup(&dir);
    }

    #[test]
    fn restarting_mid_file_keeps_the_new_points_after_the_existing_tail() {
        let dir = fixture("reopen");
        let store = HistoryStore::new(&dir);
        let base = 1_700_000_000_000;
        {
            let mut first = HistoryRecorder::open(store.clone());
            assert!(first.observe(point(base, 10.0)));
            assert!(first.observe(point(base + 20_000, 20.0)));
        }
        // 重新打开（模拟应用重启）：基准取文件尾部，不足间隔的新点照样被节流
        let mut second = HistoryRecorder::open(store.clone());
        assert!(!second.observe(point(base + 25_000, 30.0)));
        assert!(second.observe(point(base + 30_000, 40.0)));
        let read = store.read_all();
        let ts: Vec<u64> = read.points.iter().map(|p| p.t).collect();
        assert_eq!(ts, vec![base, base + 20_000, base + 30_000]);
        cleanup(&dir);
    }

    /// 前后端字段名与命令名对不上时，历史曲线会静默变空，所以在这里锁死。
    #[test]
    fn history_contract_matches_the_frontend_ipc_file() {
        let dir = fixture("contract");
        let page = query(&HistoryStore::new(&dir), 0, MIN_SPAN_SECS);
        let json = serde_json::to_string(&page).unwrap();
        let page_keys = [
            "points",
            "spanSeconds",
            "bucketSeconds",
            "storedPoints",
            "oldestMs",
            "newestMs",
            "unreadableLines",
        ];
        for key in page_keys {
            assert!(
                json.contains(&format!("\"{key}\"")),
                "HistoryPage 缺字段 {key}: {json}"
            );
        }
        assert!(!json.contains('_'), "序列化必须全 camelCase: {json}");

        let point_keys = ["t", "cpu", "memory", "rxBytesPerSec", "txBytesPerSec"];
        let point_json = serde_json::to_string(&point(1, 2.0)).unwrap();
        for key in point_keys {
            assert!(
                point_json.contains(&format!("\"{key}\"")),
                "HistoryPoint 缺字段 {key}: {point_json}"
            );
        }

        let ts = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/ipc_contract.ts");
        let contract = fs::read_to_string(&ts).expect("前端 IPC 契约文件必须存在");
        assert!(
            contract.contains("getHistory: \"get_history\""),
            "前端未登记命令 get_history"
        );
        for key in page_keys {
            assert!(
                contract.contains(&format!("{key}:")),
                "前端 HistoryPage 缺字段 {key}"
            );
        }
        for key in point_keys {
            assert!(
                contract.contains(&format!("{key}:")),
                "前端 HistoryPoint 缺字段 {key}"
            );
        }
        cleanup(&dir);
    }
}
