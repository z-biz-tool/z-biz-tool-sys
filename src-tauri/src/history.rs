//! 趋势历史与告警历史落盘（T3-07、T5-03）。
//!
//! 为什么要有这个模块：实时曲线只存在前端内存里，重启后 1 小时窗口要重新攒一小时数据，
//! "看刚才那波占用去哪了"这个问题永远答不上来。这里每 10 s 落一个点、保留 7 天；
//! 告警触发记录另存一份、保留 30 天（04 允许持久化的四类之一：偏好 / 趋势聚合 / 告警记录 / Agent 脱敏对话）。
//!
//! 刻意不引入 SQLite/chrono：单文件 JSON Lines 追加 + 按保留期重写已经够用，
//! 而依赖表每多一项，release 体积与 A-08 常驻内存目标都更难看。

use crate::alert::AlertEvent;
use crate::log_sanitize::sanitize;
use crate::monitor::{epoch_ms, MetricsSnapshot};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::marker::PhantomData;
use std::path::PathBuf;

/// 目录/文件名由后端固定生成，不接受任何外部输入，避免路径注入面。
pub const HISTORY_DIR_NAME: &str = "history";
pub const HISTORY_FILE_NAME: &str = "points.jsonl";
pub const ALERT_DIR_NAME: &str = "alerts";
pub const ALERT_FILE_NAME: &str = "alerts.jsonl";

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
pub struct HistoryStore(Jsonl<HistoryPoint>);

/// 一次全量读取的结果：点按时间递增（文件本身按追加顺序写入），外加坏行计数。
#[derive(Debug, Default)]
pub struct ReadAll {
    pub points: Vec<HistoryPoint>,
    pub unreadable_lines: usize,
    /// `None` 表示文件不存在（首次运行），这不算失败
    pub io_error: Option<String>,
}

/// 单文件 JSON Lines 底座。趋势点与告警记录共用同一套口径：
/// 每次写单独开句柄（10 s 一次的频率，省下的开销远不如"长持句柄会写到已改名的旧 inode"带来的麻烦），
/// 坏行只跳过并计数（半行是崩溃留下的，不是数据），修剪走 `.tmp` + rename（读取侧要么旧要么新，不会读半截）。
#[derive(Debug, Clone)]
pub struct Jsonl<T> {
    path: PathBuf,
    _record: PhantomData<T>,
}

#[derive(Debug)]
pub struct JsonlRead<T> {
    pub records: Vec<T>,
    pub unreadable_lines: usize,
    pub io_error: Option<String>,
}

/// 手写而不是 `#[derive(Default)]`：derive 会给泛型参数强加一个用不上的 `T: Default` 约束。
impl<T> Default for JsonlRead<T> {
    fn default() -> Self {
        Self {
            records: Vec::new(),
            unreadable_lines: 0,
            io_error: None,
        }
    }
}

impl<T: Serialize + DeserializeOwned> Jsonl<T> {
    pub fn new(dir: impl Into<PathBuf>, file_name: &str) -> Self {
        Self {
            path: dir.into().join(file_name),
            _record: PhantomData,
        }
    }

    fn ensure_parent(&self) -> std::io::Result<()> {
        let dir = self
            .path
            .parent()
            .ok_or_else(|| std::io::Error::other("历史路径没有父目录"))?;
        fs::create_dir_all(dir)
    }

    pub fn append(&self, record: &T) -> std::io::Result<()> {
        self.ensure_parent()?;
        let mut file = OpenOptions::new().create(true).append(true).open(&self.path)?;
        let line = serde_json::to_string(record)
            .map_err(|e| std::io::Error::other(format!("序列化失败: {e}")))?;
        writeln!(file, "{line}")
    }

    /// 解析失败的行只计数，绝不补零 —— 界面据此显示"有 N 行读不出"，而不是看到一条假记录。
    pub fn read(&self) -> JsonlRead<T> {
        let mut out = JsonlRead::default();
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
            match serde_json::from_str::<T>(&line) {
                Ok(record) => out.records.push(record),
                Err(_) => out.unreadable_lines += 1,
            }
        }
        out
    }

    /// 重写整个文件（修剪用）。
    pub fn rewrite(&self, records: &[T]) -> std::io::Result<()> {
        self.ensure_parent()?;
        let tmp = self.path.with_extension("tmp");
        {
            let mut file = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&tmp)?;
            for record in records {
                let line = serde_json::to_string(record)
                    .map_err(|e| std::io::Error::other(format!("序列化失败: {e}")))?;
                writeln!(file, "{line}")?;
            }
            file.flush()?;
        }
        fs::rename(&tmp, &self.path)
    }
}

impl HistoryStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self(Jsonl::new(dir, HISTORY_FILE_NAME))
    }

    pub fn append(&self, point: HistoryPoint) -> std::io::Result<()> {
        self.0.append(&point)
    }

    pub fn read_all(&self) -> ReadAll {
        let read = self.0.read();
        ReadAll {
            points: read.records,
            unreadable_lines: read.unreadable_lines,
            io_error: read.io_error,
        }
    }

    pub fn rewrite(&self, points: &[HistoryPoint]) -> std::io::Result<()> {
        self.0.rewrite(points)
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
        if let Some(err) = &read.io_error {
            // 整个文件打不开时 `points` 是空的，照常修剪等于用空集合覆盖历史文件 ——
            // 那会把"暂时读不到"变成"永久没了"。宁可这一轮不修剪。
            eprintln!("[history] 读取失败，跳过修剪: {err}");
            return 0;
        }
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

/// 告警记录（T5-03）的保留窗口：30 天，比趋势点的 7 天长。
/// 理由是两者用途不同 —— 趋势点看"刚才那波去哪了"，告警要看"上周三夜里那次满载"。
pub const ALERT_RETENTION_MS: u64 = 30 * 24 * 3600 * 1000;
/// 条数硬上限。没有它，"60 s 冷却 × 3 指标 × 多分区"最坏情况下一天就能攒上千条，
/// 保留 30 天等于不封顶。
pub const MAX_ALERT_RECORDS: usize = 5_000;
/// 触顶后一次剪到低水位：若只剪到上限本身，之后每追加一条都会触发一次全量重写。
const ALERT_PRUNE_TO: usize = MAX_ALERT_RECORDS * 9 / 10;
/// 告警查询的跨度上界 = 保留窗口；下界沿用趋势查询的 60 s。
pub const MAX_ALERT_SPAN_SECS: u64 = ALERT_RETENTION_MS / 1000;
/// 前端未指定跨度时默认看最近 7 天。
pub const DEFAULT_ALERT_SPAN_SECS: u64 = 7 * 24 * 3600;
/// 单次响应最多返回的条数。窗口最坏能攒上千条，全量过 IPC 既拖慢渲染也没人往下翻。
/// 截断不藏着：`storedEvents` 仍是窗口内的真实条数，界面据此说"还有 N 条未列出"。
pub const MAX_ALERT_PAGE_EVENTS: usize = 300;

#[derive(Debug, Clone)]
pub struct AlertStore(Jsonl<AlertEvent>);

impl AlertStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self(Jsonl::new(dir, ALERT_FILE_NAME))
    }

    fn append(&self, event: &AlertEvent) -> std::io::Result<()> {
        self.0.append(&sanitized(event))
    }

    fn read(&self) -> JsonlRead<AlertEvent> {
        self.0.read()
    }

    fn rewrite(&self, events: &[AlertEvent]) -> std::io::Result<()> {
        self.0.rewrite(events)
    }
}

/// 落盘前对挂载点过一次 `sanitize`。
/// 推送给界面的事件不改（界面要拿真实路径回答"是哪块盘"），只有**写进文件**的这份按日志口径脱敏：
/// 外接卷可能挂在 `/Users/<名字>/…` 之下，落盘文件是会长期留下的东西。
fn sanitized(event: &AlertEvent) -> AlertEvent {
    let mut copy = event.clone();
    copy.target = event.target.as_ref().map(|t| sanitize(t));
    copy
}

/// `get_alert_history` 的返回。`storedEvents`/`totalEvents` 分开的理由与趋势页一致：
/// 调用方必须能分辨"这 7 天真只有 3 条告警"和"文件里 5 000 条、窗口里 20 条"。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlertHistoryPage {
    /// 按时间倒序（最新在前），与界面列表同一读法
    pub events: Vec<AlertEvent>,
    pub span_seconds: u64,
    /// 落在请求窗口内的事件数 —— **截断之前**的数，所以它会大于 `events.len()`，那不是对不上
    pub stored_events: usize,
    /// 文件里现存的全部事件数（修剪之后、本次查询窗口之外的那些）
    pub total_events: usize,
    pub oldest_ms: Option<u64>,
    pub newest_ms: Option<u64>,
    pub unreadable_lines: usize,
}

/// 修剪间隔检查的节奏：告警本身可能几十分钟一条，不必每条都整读一遍文件判断"有没有过期"。
const ALERT_PRUNE_CHECK_MS: u64 = 6 * 3600 * 1000;

/// 采集循环里的告警落盘器：只在真的触发时追加一条，并按保留窗口 + 条数上限修剪。
///
/// 与 [`HistoryRecorder`] 的关键差别：**不丢时间戳倒退的事件**。趋势点丢一帧只是曲线上
/// 一个小缺口，而告警是稀疏事件，一条就是用户会回头查的那一次；时钟回调时宁可让文件里
/// 出现一次乱序（查询侧按时间戳排序，不信任追加顺序），也不抹掉真实发生过的告警。
pub struct AlertRecorder {
    store: AlertStore,
    newest_ms: Option<u64>,
    /// 文件里现存多少条：靠它决定"要不要修剪"，否则每条告警都要先整读一遍文件
    on_disk: usize,
    last_prune_ms: Option<u64>,
    retention_ms: u64,
    max_records: usize,
    prune_to: usize,
    check_every_ms: u64,
    pub write_failures: u64,
    pub pruned_records: usize,
}

impl AlertRecorder {
    pub fn open(store: AlertStore) -> Self {
        Self::with_limits(
            store,
            ALERT_RETENTION_MS,
            MAX_ALERT_RECORDS,
            ALERT_PRUNE_TO,
            ALERT_PRUNE_CHECK_MS,
        )
    }

    /// 上限单独可注入：生产用 `open()` 的常量，测试要验证"触顶就剪"不该真写五千条。
    fn with_limits(
        store: AlertStore,
        retention_ms: u64,
        max_records: usize,
        prune_to: usize,
        check_every_ms: u64,
    ) -> Self {
        let read = store.read();
        Self {
            store,
            newest_ms: read.records.iter().map(|e| e.timestamp_ms).max(),
            on_disk: read.records.len(),
            last_prune_ms: None,
            retention_ms,
            max_records,
            prune_to: prune_to.min(max_records),
            check_every_ms,
            write_failures: 0,
            pruned_records: 0,
        }
    }

    /// 记录一条已触发的事件，返回是否真的落盘。修剪失败不影响这条已经写进去。
    pub fn record(&mut self, event: &AlertEvent) -> bool {
        match self.store.append(event) {
            Ok(()) => {
                self.on_disk += 1;
                self.newest_ms = Some(
                    self.newest_ms
                        .unwrap_or(event.timestamp_ms)
                        .max(event.timestamp_ms),
                );
                if self.pruning_is_due() {
                    // 修剪以"见过的最新一帧"为基准，而不是这条事件自己的时间：
                    // 时钟回调时按旧时间修剪会把刚落盘的近期记录当新鲜的，判断反而错。
                    let now = self.newest_ms.unwrap_or(event.timestamp_ms);
                    self.pruned_records += self.prune_if_needed(now);
                }
                true
            }
            Err(e) => {
                self.write_failures += 1;
                if self.write_failures == 1 {
                    eprintln!("[alert] 历史写入失败: {}", sanitize(&e.to_string()));
                }
                false
            }
        }
    }

    /// 触顶必须立刻处理（那是文件尺寸的硬约束）；过期检查则按节奏做。
    fn pruning_is_due(&self) -> bool {
        if self.on_disk > self.max_records {
            return true;
        }
        let Some(now) = self.newest_ms else {
            return false;
        };
        match self.last_prune_ms {
            // 检查节奏不比保留窗口更粗：否则注入的短保留期测试永远等不到第二次检查
            Some(last) => {
                now.saturating_sub(last) >= self.check_every_ms.min(self.retention_ms)
            }
            None => true,
        }
    }

    /// 只在"确实过期或触顶"时重写文件；`now_ms` 用事件自带时间而非挂钟，
    /// 免得手动把时钟调过去就再也修剪不动。
    fn prune_if_needed(&mut self, now_ms: u64) -> usize {
        self.last_prune_ms = Some(now_ms);
        let read = self.store.read();
        if let Some(err) = &read.io_error {
            // 同趋势侧：读不出来时 `records` 是空的，继续修剪会用空集合覆盖整个告警文件。
            eprintln!("[alert] 读取失败，跳过修剪: {err}");
            return 0;
        }
        let total = read.records.len();
        let mut kept: Vec<AlertEvent> = read
            .records
            .into_iter()
            .filter(|e| now_ms.saturating_sub(e.timestamp_ms) < self.retention_ms)
            .collect();
        kept.sort_by_key(|e| e.timestamp_ms);
        if kept.len() > self.max_records {
            // 触顶时保留"最近的 N 条"，并且一次剪到低水位：
            // 只剪到上限本身会让之后每条都触发一次全量重写。
            kept = kept.split_off(kept.len() - self.prune_to);
        }
        let dropped = total.saturating_sub(kept.len());
        if dropped == 0 {
            // 计数与文件不一致（例如外部删过文件）时以文件为准，别把 `on_disk` 越写越偏
            self.on_disk = total;
            return 0;
        }
        match self.store.rewrite(&kept) {
            Ok(()) => {
                self.on_disk = kept.len();
                dropped
            }
            Err(e) => {
                eprintln!("[alert] 历史修剪失败: {}", sanitize(&e.to_string()));
                0
            }
        }
    }
}

/// 查询 `[now_ms - span, now_ms]` 内的告警记录，倒序返回。
///
/// 没有分桶压缩这一层：告警是离散事件，把两条不同指标的告警平均成一条没有意义。
pub fn query_alerts(store: &AlertStore, now_ms: u64, span_secs: u64) -> AlertHistoryPage {
    let span_secs = span_secs.clamp(MIN_SPAN_SECS, MAX_ALERT_SPAN_SECS);
    let read = store.read();
    let total_events = read.records.len();
    let from = now_ms.saturating_sub(span_secs * 1000);
    let mut matched: Vec<AlertEvent> = read
        .records
        .into_iter()
        .filter(|e| e.timestamp_ms >= from)
        .collect();
    let oldest_ms = matched.iter().map(|e| e.timestamp_ms).min();
    let newest_ms = matched.iter().map(|e| e.timestamp_ms).max();
    //  newest-first：文件里可能因时钟回调出现乱序追加，这里按时间戳重排而不是信任追加顺序
    matched.sort_by_key(|event| std::cmp::Reverse(event.timestamp_ms));
    let stored_events = matched.len();
    // 只截返回列表，不截 `stored_events`/`oldest_ms` 这些描述窗口本身的数：
    // 截完之后再说"窗口里就这 300 条"，等于把更早的那批说成不存在。
    matched.truncate(MAX_ALERT_PAGE_EVENTS);
    AlertHistoryPage {
        events: matched,
        span_seconds: span_secs,
        stored_events,
        total_events,
        oldest_ms,
        newest_ms,
        unreadable_lines: read.unreadable_lines,
    }
}

/// 与 [`store_for`] 同一套：解析失败只打印一次脱敏日志并返回 `None`，此时告警仅内存展示。
pub fn alert_store_for(app: &tauri::AppHandle) -> Option<AlertStore> {
    use tauri::Manager;
    let dir = match app.path().app_data_dir() {
        Ok(dir) => dir.join(ALERT_DIR_NAME),
        Err(e) => {
            eprintln!(
                "[alert] 无法解析应用数据目录: {}",
                sanitize(&e.to_string())
            );
            return None;
        }
    };
    if let Err(e) = fs::create_dir_all(&dir) {
        eprintln!(
            "[alert] 创建 {} 失败: {}",
            sanitize(&dir.display().to_string()),
            sanitize(&e.to_string())
        );
        return None;
    }
    Some(AlertStore::new(dir))
}

/// managed state：`None` 时告警只在会话内展示，`get_alert_history` 明确报"存储不可用"。
#[derive(Debug, Clone, Default)]
pub struct AlertHistoryState(pub Option<AlertStore>);

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

    // ---- T5-03 告警历史 ----

    use crate::alert::{AlertEvent, AlertLevel, AlertMetric};

    fn event(
        t_ms: u64,
        metric: AlertMetric,
        level: AlertLevel,
        value: f64,
        target: Option<&str>,
    ) -> AlertEvent {
        AlertEvent {
            metric,
            level,
            value,
            threshold: 95.0,
            target: target.map(|t| t.to_string()),
            consecutive: 3,
            timestamp_ms: t_ms,
        }
    }

    #[test]
    fn alert_records_survive_a_restart_and_come_back_newest_first() {
        let dir = fixture("alerts-restart");
        let store = AlertStore::new(&dir);
        let base = 1_700_000_000_000;
        {
            let mut recorder = AlertRecorder::open(store.clone());
            assert!(recorder.record(&event(
                base,
                AlertMetric::Disk,
                AlertLevel::Critical,
                98.72,
                Some("/System/Volumes/Data")
            )));
            assert!(recorder.record(&event(
                base + 60_000,
                AlertMetric::Cpu,
                AlertLevel::Warning,
                88.5,
                None
            )));
            assert_eq!(recorder.write_failures, 0);
        }
        // 重新打开（模拟应用重启）：历史必须还在，且按时间倒序回给界面
        let reopened = AlertRecorder::open(store.clone());
        assert_eq!(reopened.on_disk, 2, "重开时应把文件里的条数读回来");
        let page = query_alerts(&store, base + 120_000, 3600);
        let ts: Vec<u64> = page.events.iter().map(|e| e.timestamp_ms).collect();
        assert_eq!(ts, vec![base + 60_000, base]);
        assert_eq!(page.stored_events, 2);
        assert_eq!(page.total_events, 2);
        assert_eq!(page.oldest_ms, Some(base));
        assert_eq!(page.newest_ms, Some(base + 60_000));
        // 字段必须逐值回读，否则历史列表会显示一份和推送时不同的数
        let disk = &page
            .events
            .iter()
            .find(|e| e.metric == AlertMetric::Disk)
            .unwrap();
        assert_eq!((disk.value, disk.threshold, disk.consecutive), (98.72, 95.0, 3));
        assert_eq!(disk.level, AlertLevel::Critical);
        assert_eq!(
            disk.target.as_deref(),
            Some("/System/Volumes/Data"),
            "普通挂载点不该被改写"
        );
        assert_eq!(
            page.events.iter().find(|e| e.metric == AlertMetric::Cpu).unwrap().target,
            None,
            "CPU 告警没有挂载点，读回来也不该冒出一个空串"
        );
        cleanup(&dir);
    }

    /// 与趋势点刻意的不同：时钟回调时**不丢**告警。
    /// 丢一帧点只是曲线上一个小缺口，丢一条告警是抹掉一次真实发生过的事件。
    #[test]
    fn an_older_timestamp_still_records_the_alert_and_query_resorts() {
        let dir = fixture("alerts-clock");
        let store = AlertStore::new(&dir);
        let mut recorder = AlertRecorder::open(store.clone());
        let base = 1_700_000_000_000;
        assert!(recorder.record(&event(base + 30_000, AlertMetric::Cpu, AlertLevel::Warning, 91.0, None)));
        // 时钟回调 1 小时后的帧
        assert!(
            recorder.record(&event(
                base + 30_000 - 3_600_000,
                AlertMetric::Memory,
                AlertLevel::Critical,
                96.0,
                None
            )),
            "时间戳倒退的告警也必须留下"
        );
        // 窗口取 2 小时才能把回调那条也框进来（回调那条落在 1 小时窗口之外）
        let page = query_alerts(&store, base + 60_000, 7200);
        assert_eq!(page.events.len(), 2);
        assert!(
            page.events.windows(2).all(|w| w[0].timestamp_ms >= w[1].timestamp_ms),
            "查询要按时间戳重排，不能信任追加顺序"
        );
        assert_eq!(page.newest_ms, Some(base + 30_000));
        assert_eq!(page.oldest_ms, Some(base + 30_000 - 3_600_000));
        assert_eq!(page.total_events, 2);
        // 窗口收窄时那条"更旧"的记录只是不在窗口里，不是被丢了：两个数必须分得开
        let narrow = query_alerts(&store, base + 60_000, 3600);
        assert_eq!(narrow.stored_events, 1);
        assert_eq!(narrow.total_events, 2, "文件里现存两条，不能因为窗口窄就说成一条");
        // 修剪基准取"见过的最新一帧"，那条更旧的近期记录不该被当成过期剪掉
        assert_eq!(store.read().records.len(), 2);
        cleanup(&dir);
    }

    #[test]
    fn persisted_alert_targets_are_sanitized_while_the_event_itself_is_not() {
        let dir = fixture("alerts-sanitize");
        let store = AlertStore::new(&dir);
        let raw = "/Users/tester/MyPassport";
        let base = 1_700_000_000_000;
        let original = event(base, AlertMetric::Disk, AlertLevel::Warning, 92.0, Some(raw));
        assert!(AlertRecorder::open(store.clone()).record(&original));
        // 落盘那份过 sanitize（文件会长期留在磁盘上），推给界面的那份保持原样
        let on_disk = fs::read_to_string(dir.join(ALERT_FILE_NAME)).unwrap();
        assert!(on_disk.contains("/Users/<user>/MyPassport"), "实际内容: {on_disk}");
        assert!(!on_disk.contains("tester"), "用户名不得进文件");
        assert_eq!(original.target.as_deref(), Some(raw), "内存里的事件不该被改写");
        let page = query_alerts(&store, base, 3600);
        assert_eq!(page.events[0].target.as_deref(), Some("/Users/<user>/MyPassport"));
        cleanup(&dir);
    }

    #[test]
    fn alerts_beyond_the_cap_keep_the_newest_and_prune_only_once_to_a_low_watermark() {
        let dir = fixture("alerts-cap");
        let store = AlertStore::new(&dir);
        let mut recorder = AlertRecorder::with_limits(store.clone(), ALERT_RETENTION_MS, 20, 15, 1);
        let base = 1_700_000_000_000;
        for i in 0..21 {
            assert!(recorder.record(&event(
                base + i * 1_000,
                AlertMetric::Cpu,
                AlertLevel::Warning,
                90.0,
                None
            )));
        }
        assert!(
            recorder.pruned_records > 0,
            "超过条数上限必须剪"
        );
        let read = store.read();
        assert!(
            read.records.len() <= 15,
            "应一次剪到低水位，实际 {}",
            read.records.len()
        );
        assert!(
            read.records.last().map(|e| e.timestamp_ms) == Some(base + 20_000),
            "剪的是最旧的，最新一条不能丢"
        );
        assert!(
            read.records.iter().all(|e| e.timestamp_ms >= base + 5_000),
            "保留的必须是最靠后的那批"
        );
        // 修剪后的条数要被前端看见（totalEvents 是"现存"，不是"历史上写过"）
        let page = query_alerts(&store, base + 20_000, 3600);
        assert_eq!(page.total_events, read.records.len());
        assert_eq!(page.stored_events, read.records.len());
        cleanup(&dir);
    }

    #[test]
    fn alerts_older_than_the_retention_window_are_pruned() {
        let dir = fixture("alerts-retention");
        let store = AlertStore::new(&dir);
        let base = 1_700_000_000_000;
        store
            .append(&event(base, AlertMetric::Cpu, AlertLevel::Critical, 99.0, None))
            .unwrap();
        let mut recorder = AlertRecorder::with_limits(store.clone(), 60_000, 100, 90, 1);
        // 10 分钟后的一条：上一条已超出 60 s 的保留窗口
        assert!(recorder.record(&event(
            base + 600_000,
            AlertMetric::Memory,
            AlertLevel::Warning,
            90.0,
            None
        )));
        assert_eq!(recorder.pruned_records, 1, "过期的一条应被剪掉");
        let read = store.read();
        assert_eq!(read.records.len(), 1);
        assert_eq!(read.records[0].metric, AlertMetric::Memory);
        cleanup(&dir);
    }

    #[test]
    fn a_broken_alert_line_is_counted_not_repaired() {
        let dir = fixture("alerts-broken");
        let store = AlertStore::new(&dir);
        let base = 1_700_000_000_000;
        store
            .append(&event(base, AlertMetric::Cpu, AlertLevel::Warning, 90.0, None))
            .unwrap();
        {
            // 模拟进程被强杀时只剩半行的尾部
            let mut f = OpenOptions::new()
                .append(true)
                .open(dir.join(ALERT_FILE_NAME))
                .unwrap();
            f.write_all(b"{\"metric\":\"cpu\",\"tim").unwrap();
            f.write_all(b"\n").unwrap();
        }
        store
            .append(&event(base + 5_000, AlertMetric::Disk, AlertLevel::Critical, 99.0, None))
            .unwrap();
        let page = query_alerts(&store, base + 5_000, 3600);
        assert_eq!(page.events.len(), 2, "坏行前后的完整记录仍要读出来");
        assert_eq!(page.unreadable_lines, 1, "坏行数要透到响应里，不能静默吞掉");
        cleanup(&dir);
    }

    #[test]
    fn an_empty_or_missing_alert_file_reports_nothing_instead_of_inventing_events() {
        let dir = fixture("alerts-empty");
        let page = query_alerts(&AlertStore::new(dir.join("nope")), 1_700_000_000_000, 3600);
        assert!(page.events.is_empty());
        assert_eq!(page.stored_events, 0);
        assert_eq!(page.total_events, 0);
        assert_eq!(page.oldest_ms, None);
        assert_eq!(page.newest_ms, None);
        assert_eq!(page.unreadable_lines, 0);
        assert_eq!(page.span_seconds, 3600);
        cleanup(&dir);
    }

    /// 跨度夹取与"只报告真实覆盖"：界面不能因为选了 30 天就把一次 2 分钟的历史显示成满窗。
    #[test]
    fn alert_query_clamps_the_span_and_exposes_the_real_gap() {
        let dir = fixture("alerts-span");
        let store = AlertStore::new(&dir);
        let base = 1_700_000_000_000;
        store
            .append(&event(base, AlertMetric::Cpu, AlertLevel::Warning, 90.0, None))
            .unwrap();
        let huge = query_alerts(&store, base, MAX_ALERT_SPAN_SECS * 4);
        assert_eq!(huge.span_seconds, MAX_ALERT_SPAN_SECS, "跨度应夹到保留窗口");
        let tiny = query_alerts(&store, base, 1);
        assert_eq!(tiny.span_seconds, MIN_SPAN_SECS, "跨度应夹到下限");
        // 窗口之外的记录不进 events，但仍计入 totalEvents
        let later = query_alerts(&store, base + 2 * 3_600_000, 3600);
        assert_eq!(later.events.len(), 0);
        assert_eq!(later.stored_events, 0);
        assert_eq!(later.total_events, 1, "文件里明明有一条，界面得能说出来");
        assert_eq!(later.oldest_ms, None, "窗口内没有点就不能报窗口外的时间戳");
        cleanup(&dir);
    }

    #[test]
    fn an_overlong_alert_window_is_truncated_without_losing_the_real_count() {
        let dir = fixture("alerts-cap");
        let store = AlertStore::new(&dir);
        let base = 1_700_000_000_000;
        let total = MAX_ALERT_PAGE_EVENTS + 7;
        for i in 0..total {
            store
                .append(&event(
                    base + i as u64 * 1_000,
                    AlertMetric::Cpu,
                    AlertLevel::Warning,
                    90.0,
                    None,
                ))
                .unwrap();
        }

        let page = query_alerts(&store, base + total as u64 * 1_000, 3_600);
        assert_eq!(page.events.len(), MAX_ALERT_PAGE_EVENTS);
        assert_eq!(page.stored_events, total, "截短的是列表，不是窗口里的条数");
        assert_eq!(
            page.events[0].timestamp_ms,
            base + (total as u64 - 1) * 1_000,
            "被截掉的必须是更早的那批，不是随机一段"
        );
        assert_eq!(
            page.oldest_ms,
            Some(base),
            "窗口覆盖范围不能因为列表截断就说成从第 300 条才开始"
        );
        cleanup(&dir);
    }

    /// 命令名与字段名对不上时，历史列表会静默变空，所以和 T5-01 的契约测试同一手法锁死。
    #[test]
    fn alert_history_contract_matches_the_frontend_ipc_file() {
        let dir = fixture("alerts-contract");
        let page = query_alerts(&AlertStore::new(&dir), 0, MIN_SPAN_SECS);
        let json = serde_json::to_string(&page).unwrap();
        let page_keys = [
            "events",
            "spanSeconds",
            "storedEvents",
            "totalEvents",
            "oldestMs",
            "newestMs",
            "unreadableLines",
        ];
        for key in page_keys {
            assert!(
                json.contains(&format!("\"{key}\"")),
                "AlertHistoryPage 缺字段 {key}: {json}"
            );
        }
        assert!(!json.contains('_'), "序列化必须全 camelCase: {json}");

        let ts = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/ipc_contract.ts");
        let contract = fs::read_to_string(&ts).expect("前端 IPC 契约文件必须存在");
        assert!(
            contract.contains("getAlertHistory: \"get_alert_history\""),
            "前端未登记命令 get_alert_history"
        );
        for key in page_keys {
            assert!(
                contract.contains(&format!("{key}:")),
                "前端 AlertHistoryPage 缺字段 {key}"
            );
        }
        cleanup(&dir);
    }

    /// 只按 unix 权限位制造"文件存在但打不开"。以 root 跑测试时权限位不生效，
    /// 调用方须先探测再断言，免得把"没触发到这条路径"记成"测过了"。
    fn chmod(file: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(file, fs::Permissions::from_mode(mode)).unwrap();
    }

    fn open_blocked(file: &Path) -> bool {
        chmod(file, 0o000);
        let blocked = fs::File::open(file).is_err();
        chmod(file, 0o644);
        blocked
    }

    #[test]
    fn a_history_file_that_cannot_be_read_is_not_overwritten_by_pruning() {
        let dir = fixture("history-unreadable");
        let store = HistoryStore::new(&dir);
        let base = 1_700_000_000_000;
        for i in 0..4 {
            store
                .append(point(base + i * SAMPLE_INTERVAL_MS, i as f64))
                .unwrap();
        }
        // 先以可读状态打开记录器，让 oldest_ms 真的被填上 —— 否则"过期判定"根本走不到读取那一步
        let mut recorder = HistoryRecorder::open(store.clone());
        let file = dir.join(HISTORY_FILE_NAME);
        if !open_blocked(&file) {
            eprintln!("[skip] 当前用户无视权限位，无法制造读取失败");
            cleanup(&dir);
            return;
        }
        chmod(&file, 0o000);
        let dropped = recorder.prune_if_expired(base + RETENTION_MS + 1);
        chmod(&file, 0o644);

        assert_eq!(dropped, 0, "读不出任何点，就没有资格声称修剪掉了若干点");
        assert_eq!(
            store.read_all().points.len(),
            4,
            "读取失败时 records 是空的，照常修剪等于用空集合覆盖整个历史文件"
        );
        cleanup(&dir);
    }

    #[test]
    fn an_alert_file_that_cannot_be_read_leaves_the_counter_and_the_file_alone() {
        let dir = fixture("alerts-unreadable");
        let store = AlertStore::new(&dir);
        let base = 1_700_000_000_000;
        for i in 0..3 {
            store
                .append(&event(
                    base + i * 1_000,
                    AlertMetric::Cpu,
                    AlertLevel::Warning,
                    90.0,
                    None,
                ))
                .unwrap();
        }
        let mut recorder = AlertRecorder::with_limits(store.clone(), 60_000, 10, 8, 1_000);
        assert_eq!(recorder.on_disk, 3);
        let file = dir.join(ALERT_FILE_NAME);
        if !open_blocked(&file) {
            eprintln!("[skip] 当前用户无视权限位，无法制造读取失败");
            cleanup(&dir);
            return;
        }
        chmod(&file, 0o000);
        let dropped = recorder.prune_if_needed(base + RETENTION_MS + 1);
        chmod(&file, 0o644);

        assert_eq!(dropped, 0);
        assert_eq!(
            recorder.on_disk,
            3,
            "读不出来时把计数当 0，之后触顶判断会以为文件还早着呢"
        );
        assert_eq!(store.read().records.len(), 3);
        cleanup(&dir);
    }
}
