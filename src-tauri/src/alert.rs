//! T5-01 告警引擎：阈值配置 + 连续超限检测 + 去抖冷却。
//!
//! 口径（06 的 UT-15 / UT-16 逐条锁死）：
//! - 连续第 `consecutive` 帧仍然越限才触发，中途任何一帧回落就把该档计数清零（不累计）；
//! - warning 与 critical 各有一条独立计数，所以 96 % 的帧也给 warning 计数加一，
//!   但要报 critical 必须 critical 自己攒满 N 帧；两档都在攒时取更高的那一档；
//! - 回落到 warning 之下算"恢复"，连同冷却一起清空，下一次异常不必再等冷却；
//! - 同一 `(指标, 目标, 档位)` 在 `cooldown_secs` 内不重复推送，避免每秒刷屏。
//!
//! 时间全部取帧自带的 `timestamp_ms`（采集器打的戳），不读挂钟：
//! 这样冷却可测、且与历史落盘用的是同一个时间轴。
//!
//! 本模块只做判定与阈值口径；触发出来的 `AlertEvent` 由 `history::AlertRecorder` 落盘（T5-03）。

use crate::monitor::{DiskMetrics, MetricsSnapshot};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// 后端 → 前端的告警事件名；前端按 `AlertEvent` 解析。
pub const ALERT_EVENT: &str = "sys://alert";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AlertMetric {
    Cpu,
    Memory,
    Disk,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AlertLevel {
    Warning,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AlertThresholds {
    pub warning: f64,
    pub critical: f64,
}

/// 供 `#[serde(default)]` 使用：payload 里整段缺失时取这个中性值。
/// 刻意不是 0 —— 0 % 阈值等于"每帧都告警"。各指标自己的默认值在 [`AlertConfig::default`]。
impl Default for AlertThresholds {
    fn default() -> Self {
        Self::new(90.0, 95.0)
    }
}

impl AlertThresholds {
    const fn new(warning: f64, critical: f64) -> Self {
        Self { warning, critical }
    }
    /// 阈值必须落在 `1..=100`（百分比），且 critical 不得低于 warning：
    /// 否则同一帧会"越过两档"，界面上就是两条重复告警。
    /// NaN / Infinity 折算成 100（最保守的那一档），别让一个坏值把引擎变成一直刷屏。
    fn clamped(self) -> Self {
        let warning = finite_or(self.warning).clamp(1.0, 100.0);
        Self {
            warning,
            critical: finite_or(self.critical).clamp(warning, 100.0),
        }
    }
}

fn finite_or(value: f64) -> f64 {
    if value.is_finite() {
        value
    } else {
        100.0
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AlertConfig {
    pub enabled: bool,
    /// 连续多少帧越限才触发；1 即"第一帧就报"，默认 3（02 文档 F6）。
    pub consecutive: u32,
    /// 同一告警的最小重复推送间隔。
    pub cooldown_secs: u64,
    pub cpu: AlertThresholds,
    pub memory: AlertThresholds,
    pub disk: AlertThresholds,
}

impl Default for AlertConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            consecutive: 3,
            cooldown_secs: 60,
            // 02 文档只给了 warning 档默认值（80/85/90），critical 取 95。
            cpu: AlertThresholds::new(80.0, 95.0),
            memory: AlertThresholds::new(85.0, 95.0),
            disk: AlertThresholds::new(90.0, 95.0),
        }
    }
}

impl AlertConfig {
    pub fn clamped(self) -> Self {
        let cpu = self.cpu.clamped();
        let memory = self.memory.clamped();
        let disk = self.disk.clamped();
        Self {
            enabled: self.enabled,
            consecutive: self.consecutive.clamp(1, 60),
            cooldown_secs: self.cooldown_secs.clamp(0, 86_400),
            cpu,
            memory,
            disk,
        }
    }

    fn thresholds_for(&self, metric: AlertMetric) -> AlertThresholds {
        match metric {
            AlertMetric::Cpu => self.cpu,
            AlertMetric::Memory => self.memory,
            AlertMetric::Disk => self.disk,
        }
    }
}

/// 一条已触发的告警。既是 `sys://alert` 的 payload，也是 T5-03 落盘与回读的记录，
/// 所以要能反序列化 —— 读回来的字段必须和当初推出去的一模一样，否则历史列表会静默变空。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlertEvent {
    pub metric: AlertMetric,
    pub level: AlertLevel,
    /// 触发帧的实际值（百分比，两位小数）
    pub value: f64,
    /// 被越过的那一档阈值
    pub threshold: f64,
    /// 磁盘告警的挂载点；CPU / 内存为 `None`
    pub target: Option<String>,
    /// 触发时的连续越限帧数
    pub consecutive: u32,
    pub timestamp_ms: u64,
}

/// managed state：采集循环每帧读它，`get_alert_config` / `set_alert_config` 读写它。
#[derive(Clone, Default)]
pub struct AlertState(pub Arc<RwLock<AlertConfig>>);

impl AlertState {
    pub fn get(&self) -> AlertConfig {
        self.0.read().map(|g| g.clone()).unwrap_or_default()
    }

    /// 夹取后落库，并返回真正生效的配置（前端据此校正输入框）。
    pub fn set(&self, config: AlertConfig) -> AlertConfig {
        let clamped = config.clamped();
        if let Ok(mut guard) = self.0.write() {
            *guard = clamped.clone();
        }
        clamped
    }
}

#[derive(Debug, Default)]
struct KeyState {
    warning_streak: u32,
    critical_streak: u32,
    last_fired: Option<(AlertLevel, u64)>,
}

/// 每帧把 [`MetricsSnapshot`] 过一遍阈值，产出应当推送的告警。
/// 只在采集循环里单点使用，故不做 `Send`。
#[derive(Debug, Default)]
pub struct AlertEngine {
    states: HashMap<(AlertMetric, Option<String>), KeyState>,
}

impl AlertEngine {
    pub fn new() -> Self {
        Self::default()
    }

    /// 丢掉所有连续计数。采集循环在暂停和 panic 帧上调用：那两种情况下"这一帧"根本不存在，
    /// 让计数跨过空洞继续攒会把一个断续的异常报成"连续超限"。
    pub fn reset(&mut self) {
        self.states.clear();
    }

    pub fn evaluate(&mut self, snapshot: &MetricsSnapshot, config: &AlertConfig) -> Vec<AlertEvent> {
        if !config.enabled {
            // 静默期间连计数也不攒：否则关掉再打开会把静默期的旧状态当成"已连续越限"立刻弹出。
            self.reset();
            return Vec::new();
        }
        // 命令入口已经夹取过；这里再夹一次，防住直接构造 `AlertConfig` 的调用方。
        let config = config.clone().clamped();

        let mut observations: Vec<(AlertMetric, Option<String>, f64)> = vec![
            (AlertMetric::Cpu, None, snapshot.cpu.total),
            (AlertMetric::Memory, None, snapshot.memory.usage_percent),
        ];
        if let Some(disk) = worst_disk(snapshot) {
            observations.push((
                AlertMetric::Disk,
                Some(disk.mount_point.clone()),
                disk.usage_percent,
            ));
        }

        let mut events = Vec::new();
        let mut seen = Vec::with_capacity(observations.len());
        for (metric, target, value) in observations {
            let key = (metric, target.clone());
            seen.push(key.clone());
            if let Some(event) = self.observe(metric, target, value, &config, snapshot.timestamp_ms) {
                events.push(event);
            }
        }
        // 卸载/重挂载的分区不该继续占着状态（也顺带让这个 map 有界）。
        self.states.retain(|key, _| seen.contains(key));
        events
    }

    /// `config` 必须是已夹取的那份；阈值、连续帧数与冷却都从它取，避免调用方拼出错位的参数组合。
    fn observe(
        &mut self,
        metric: AlertMetric,
        target: Option<String>,
        value: f64,
        config: &AlertConfig,
        timestamp_ms: u64,
    ) -> Option<AlertEvent> {
        let thresholds = config.thresholds_for(metric);
        let consecutive = config.consecutive;
        let cooldown_secs = config.cooldown_secs;
        let state = self.states.entry((metric, target.clone())).or_default();
        state.critical_streak = if value >= thresholds.critical {
            state.critical_streak + 1
        } else {
            0
        };
        state.warning_streak = if value >= thresholds.warning {
            state.warning_streak + 1
        } else {
            0
        };
        if value < thresholds.warning {
            // 恢复：清计数也清冷却。
            *state = KeyState::default();
            return None;
        }

        let (level, threshold, streak) = if state.critical_streak >= consecutive {
            (AlertLevel::Critical, thresholds.critical, state.critical_streak)
        } else if state.warning_streak >= consecutive {
            (AlertLevel::Warning, thresholds.warning, state.warning_streak)
        } else {
            return None;
        };

        if let Some((fired_level, fired_ms)) = state.last_fired {
            let in_cooldown = fired_level == level
                && timestamp_ms.saturating_sub(fired_ms) < cooldown_secs * 1000;
            if in_cooldown {
                return None;
            }
        }
        state.last_fired = Some((level, timestamp_ms));
        Some(AlertEvent {
            metric,
            level,
            value: round2(value),
            threshold,
            target,
            consecutive: streak,
            timestamp_ms,
        })
    }
}

/// 磁盘只报最满的那一块：多分区同时刷屏对用户没有额外信息量。
fn worst_disk(snapshot: &MetricsSnapshot) -> Option<&DiskMetrics> {
    snapshot
        .disks
        .iter()
        .filter(|d| d.available && d.total_bytes > 0)
        .max_by(|a, b| a.usage_percent.total_cmp(&b.usage_percent))
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::monitor::{CpuMetrics, DiskMetrics, MemoryMetrics, MemoryPressure};

    fn snapshot_at(ts_ms: u64, cpu: f64, memory: f64, disks: &[(&str, f64)]) -> MetricsSnapshot {
        MetricsSnapshot {
            timestamp_ms: ts_ms,
            uptime_seconds: 1,
            cpu: CpuMetrics {
                total: cpu,
                per_core: vec![cpu],
                core_count: 1,
            },
            memory: MemoryMetrics {
                total_bytes: 16 * 1024 * 1024 * 1024,
                used_bytes: (memory / 100.0 * 16.0) as u64 * 1024 * 1024 * 1024,
                available_bytes: 0,
                swap_total_bytes: 0,
                swap_used_bytes: 0,
                usage_percent: memory,
                pressure: MemoryPressure::Normal,
            },
            disks: disks
                .iter()
                .map(|(mount, percent)| DiskMetrics {
                    name: mount.to_string(),
                    mount_point: mount.to_string(),
                    file_system: "apfs".to_string(),
                    total_bytes: 100,
                    used_bytes: (*percent as u64).min(100),
                    available_bytes: 0,
                    usage_percent: *percent,
                    read_bytes_per_sec: None,
                    write_bytes_per_sec: None,
                    available: true,
                })
                .collect(),
            networks: Vec::new(),
        }
    }

    fn config() -> AlertConfig {
        AlertConfig::default()
    }

    /// UT-15：连续超限去抖 —— 阈值 count=3 时，2 次超限不触发，第 3 次触发。
    #[test]
    fn fires_only_on_the_third_consecutive_over_threshold_frame() {
        let mut engine = AlertEngine::new();
        let cfg = config();

        assert!(engine
            .evaluate(&snapshot_at(1_000, 90.0, 10.0, &[]), &cfg)
            .is_empty());
        assert!(engine
            .evaluate(&snapshot_at(2_000, 91.0, 10.0, &[]), &cfg)
            .is_empty());

        let events = engine.evaluate(&snapshot_at(3_000, 92.0, 10.0, &[]), &cfg);
        assert_eq!(events.len(), 1, "第三帧应触发一条告警，实际 {events:?}");
        assert_eq!(events[0].metric, AlertMetric::Cpu);
        assert_eq!(events[0].level, AlertLevel::Warning);
        assert_eq!(events[0].consecutive, 3);
        assert_eq!(events[0].threshold, 80.0);
        assert_eq!(events[0].target, None);
        assert_eq!(events[0].timestamp_ms, 3_000);
    }

    /// UT-16：恢复后重置计数 —— 超限→正常→超限 不累计。
    #[test]
    fn a_normal_frame_resets_the_consecutive_count() {
        let mut engine = AlertEngine::new();
        let cfg = config();

        engine.evaluate(&snapshot_at(1_000, 90.0, 10.0, &[]), &cfg);
        engine.evaluate(&snapshot_at(2_000, 90.0, 10.0, &[]), &cfg);
        // 中间这一帧回到阈值之下
        assert!(engine
            .evaluate(&snapshot_at(3_000, 10.0, 10.0, &[]), &cfg)
            .is_empty());
        // 若计数没清零，这一帧就会被当成"第 3 次超限"弹出
        assert!(
            engine
                .evaluate(&snapshot_at(4_000, 90.0, 10.0, &[]), &cfg)
                .is_empty(),
            "恢复后的第一帧不得复用旧的连续计数"
        );
        assert!(engine
            .evaluate(&snapshot_at(5_000, 90.0, 10.0, &[]), &cfg)
            .is_empty());
        let events = engine.evaluate(&snapshot_at(6_000, 90.0, 10.0, &[]), &cfg);
        assert_eq!(events.len(), 1, "重新攒满 3 帧才该触发");
    }

    /// 暂停 / panic 帧没有产出数据，计数不能跨过这段空白（采集循环调 `reset`）。
    #[test]
    fn reset_breaks_the_streak() {
        let mut engine = AlertEngine::new();
        let cfg = config();
        engine.evaluate(&snapshot_at(1_000, 90.0, 10.0, &[]), &cfg);
        engine.evaluate(&snapshot_at(2_000, 90.0, 10.0, &[]), &cfg);
        engine.reset();
        assert!(
            engine
                .evaluate(&snapshot_at(3_000, 90.0, 10.0, &[]), &cfg)
                .is_empty(),
            "reset 之后的第一帧必须从 1 重新计数"
        );
    }

    #[test]
    fn warning_and_critical_keep_separate_streaks_and_critical_wins() {
        let mut engine = AlertEngine::new();
        let cfg = config();

        // 两帧 85 %（越过 warning）+ 一帧 99 %（越过 critical）：
        // critical 计数只有 1，先按 warning 档报；随后两帧攒满 critical 再升级。
        engine.evaluate(&snapshot_at(1_000, 85.0, 10.0, &[]), &cfg);
        engine.evaluate(&snapshot_at(2_000, 85.0, 10.0, &[]), &cfg);
        let first = engine.evaluate(&snapshot_at(3_000, 99.0, 10.0, &[]), &cfg);
        assert_eq!(first[0].level, AlertLevel::Warning);
        assert!(engine
            .evaluate(&snapshot_at(4_000, 99.0, 10.0, &[]), &cfg)
            .is_empty());
        let escalated = engine.evaluate(&snapshot_at(5_000, 99.0, 10.0, &[]), &cfg);
        assert_eq!(escalated.len(), 1);
        assert_eq!(escalated[0].level, AlertLevel::Critical);
        assert_eq!(escalated[0].consecutive, 3);
        assert_eq!(escalated[0].threshold, 95.0);
    }

    #[test]
    fn cooldown_suppresses_repeats_and_recovery_clears_it() {
        let mut engine = AlertEngine::new();
        let cfg = config(); // 连续 3 帧、冷却 60 s
        let hot = |ts: u64| snapshot_at(ts, 90.0, 10.0, &[("/", 10.0)]);

        engine.evaluate(&hot(1_000), &cfg);
        engine.evaluate(&hot(2_000), &cfg);
        assert_eq!(engine.evaluate(&hot(3_000), &cfg).len(), 1);
        // 持续超限但不重复刷屏
        for ts in [4_000, 10_000, 30_000] {
            assert!(
                engine.evaluate(&hot(ts), &cfg).is_empty(),
                "冷却窗口内 {ts} ms 不该再推"
            );
        }
        // 距上次触发满 60 s 后仍超限：再报一次是符合预期的
        assert_eq!(engine.evaluate(&hot(63_000), &cfg).len(), 1);

        // 恢复一次即清冷却：下一次异常 episode 不必再等 60 s
        engine.evaluate(&snapshot_at(64_000, 5.0, 5.0, &[]), &cfg);
        assert!(engine.evaluate(&hot(65_000), &cfg).is_empty());
        engine.evaluate(&hot(66_000), &cfg);
        assert_eq!(
            engine.evaluate(&hot(67_000), &cfg).len(),
            1,
            "新 episode 攒满 3 帧即报，不受上一轮冷却影响"
        );
    }

    #[test]
    fn disabled_config_emits_nothing_and_does_not_bank_counts() {
        let mut engine = AlertEngine::new();
        let off = AlertConfig {
            enabled: false,
            ..Default::default()
        };
        let hot = |ts: u64| snapshot_at(ts, 99.0, 10.0, &[("/", 10.0)]);
        for ts in [1_000, 2_000, 3_000] {
            assert!(engine.evaluate(&hot(ts), &off).is_empty());
        }
        // 打开后从头计数，不能因为静默期"已经超限 3 帧"而立刻弹
        let cfg = config();
        assert!(engine.evaluate(&hot(4_000), &cfg).is_empty());
        assert!(engine.evaluate(&hot(5_000), &cfg).is_empty());
        assert_eq!(engine.evaluate(&hot(6_000), &cfg).len(), 1);
    }

    #[test]
    fn disk_alerts_target_the_fullest_available_partition() {
        let mut engine = AlertEngine::new();
        let cfg = config();
        let mut frame = snapshot_at(
            1_000,
            1.0,
            1.0,
            &[("/SystemData", 40.0), ("/Volumes/Big", 97.0)],
        );
        // 挂载点存在但读不到用量（`available:false`）时不能拿它的陈旧百分比报故障
        frame.disks.push(DiskMetrics {
            name: "ghost".to_string(),
            mount_point: "/Volumes/Ghost".to_string(),
            file_system: "unknown".to_string(),
            total_bytes: 100,
            used_bytes: 100,
            available_bytes: 0,
            usage_percent: 100.0,
            read_bytes_per_sec: None,
            write_bytes_per_sec: None,
            available: false,
        });

        engine.evaluate(&frame, &cfg);
        engine.evaluate(&frame, &cfg);
        let events = engine.evaluate(&frame, &cfg);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].metric, AlertMetric::Disk);
        assert_eq!(events[0].target.as_deref(), Some("/Volumes/Big"));
        assert_eq!(events[0].level, AlertLevel::Critical);
    }

    #[test]
    fn each_metric_fires_independently_in_the_same_frame() {
        let mut engine = AlertEngine::new();
        let cfg = config();
        for ts in [1_000, 2_000] {
            assert!(engine
                .evaluate(&snapshot_at(ts, 95.0, 96.0, &[("/", 99.0)]), &cfg)
                .is_empty());
        }
        let events = engine.evaluate(&snapshot_at(3_000, 95.0, 96.0, &[("/", 99.0)]), &cfg);
        let metrics: Vec<AlertMetric> = events.iter().map(|e| e.metric).collect();
        assert_eq!(
            metrics,
            vec![AlertMetric::Cpu, AlertMetric::Memory, AlertMetric::Disk],
            "三个指标应在同一帧各自产出一条"
        );
        assert_eq!(events[0].level, AlertLevel::Critical);
        assert_eq!(events[1].level, AlertLevel::Critical);
    }

    #[test]
    fn config_clamps_thresholds_and_keeps_critical_above_warning() {
        let messy = AlertConfig {
            enabled: true,
            consecutive: 0,
            cooldown_secs: u64::MAX,
            cpu: AlertThresholds::new(0.0, 3.0),
            memory: AlertThresholds::new(120.0, 50.0),
            disk: AlertThresholds::new(90.0, f64::NAN),
        }
        .clamped();

        assert_eq!(messy.consecutive, 1);
        assert_eq!(messy.cooldown_secs, 86_400);
        assert_eq!(messy.cpu.warning, 1.0);
        assert_eq!(messy.cpu.critical, 3.0);
        assert_eq!(messy.memory.warning, 100.0);
        assert_eq!(
            messy.memory.critical, 100.0,
            "critical 不得低于 warning，倒挂时按 warning 收敛"
        );
        assert!(
            messy.disk.critical.is_finite() && messy.disk.critical >= messy.disk.warning,
            "NaN 阈值必须折成有限值: {:?}",
            messy.disk
        );
        assert_eq!(messy.disk.warning, 90.0);
        assert_eq!(messy.disk.critical, 100.0);
    }

    /// 跨语言契约：字段名必须与 `ipc_contract.ts` 逐字一致（与 history/cleanup 同口径）。
    #[test]
    fn alert_contract_matches_the_frontend_ipc_file() {
        let event = AlertEvent {
            metric: AlertMetric::Disk,
            level: AlertLevel::Warning,
            value: 91.23,
            threshold: 90.0,
            target: Some("/Volumes/X".to_string()),
            consecutive: 3,
            timestamp_ms: 1,
        };
        let json = serde_json::to_string(&event).unwrap();
        let event_keys = [
            "metric",
            "level",
            "value",
            "threshold",
            "target",
            "consecutive",
            "timestampMs",
        ];
        for key in event_keys {
            assert!(
                json.contains(&format!("\"{key}\"")),
                "AlertEvent 缺字段 {key}: {json}"
            );
        }
        assert!(json.contains("\"disk\""), "metric 应序列化为小写: {json}");
        assert!(json.contains("\"warning\""), "level 应序列化为小写: {json}");

        let config = AlertConfig::default();
        let cfg_json = serde_json::to_string(&config).unwrap();
        let config_keys = [
            "enabled",
            "consecutive",
            "cooldownSecs",
            "cpu",
            "memory",
            "disk",
            "warning",
            "critical",
        ];
        for key in config_keys {
            assert!(
                cfg_json.contains(&format!("\"{key}\"")),
                "AlertConfig 缺字段 {key}: {cfg_json}"
            );
        }
        assert!(
            !cfg_json.contains('_'),
            "序列化必须全 camelCase: {cfg_json}"
        );

        let contract = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/ipc_contract.ts"),
        )
        .expect("前端 IPC 契约文件必须存在");
        assert!(
            contract.contains(&format!("alert: \"{ALERT_EVENT}\"")),
            "前端未登记告警事件名 {ALERT_EVENT}"
        );
        assert!(
            contract.contains("getAlertConfig: \"get_alert_config\""),
            "前端未登记命令 get_alert_config"
        );
        assert!(
            contract.contains("setAlertConfig: \"set_alert_config\""),
            "前端未登记命令 set_alert_config"
        );
        for key in event_keys {
            assert!(
                contract.contains(&format!("{key}:")),
                "前端 AlertEvent 缺字段 {key}"
            );
        }
        for key in ["cooldownSecs", "consecutive", "enabled"] {
            assert!(
                contract.contains(&format!("{key}:")),
                "前端 AlertConfig 缺字段 {key}"
            );
        }
    }

    /// 真实采集器 → 引擎的端到端一段：证明判定读的是 `Collector` 真产出的字段，
    /// 而不是只在我手工拼的快照上成立（字段错位这类 bug 只有真帧能暴露）。
    #[test]
    fn real_collector_frames_feed_the_engine() {
        use crate::monitor::Collector;
        use std::time::Duration;

        let mut collector = Collector::new();
        collector.refresh_cpu();
        std::thread::sleep(Duration::from_millis(250));

        // 阈值压到 1 %：这台机器的内存占用必然越限，因此三帧之后就该出事件。
        // CPU / 磁盘钉在 100 %，用来核对事件读的是各自那一栏而不是串了字段。
        let cfg = AlertConfig {
            consecutive: 3,
            cooldown_secs: 0,
            memory: AlertThresholds::new(1.0, 2.0),
            cpu: AlertThresholds::new(100.0, 100.0),
            disk: AlertThresholds::new(100.0, 100.0),
            ..Default::default()
        }
        .clamped();

        let mut engine = AlertEngine::new();
        let mut fired = Vec::new();
        let mut last_memory = 0.0;
        for _ in 0..3 {
            let frame = collector.snapshot(Duration::from_millis(10_000));
            last_memory = frame.memory.usage_percent;
            fired.extend(engine.evaluate(&frame, &cfg));
            std::thread::sleep(Duration::from_millis(220));
        }

        assert_eq!(fired.len(), 1, "只有内存越限，应恰好一条: {fired:?}");
        let memory = &fired[0];
        assert_eq!(memory.metric, AlertMetric::Memory);
        assert_eq!(
            memory.value,
            round2(last_memory),
            "事件里的值必须就是这一帧的内存占用率"
        );
        assert_eq!(memory.level, AlertLevel::Critical);
        assert_eq!(memory.consecutive, 3);
        assert_eq!(memory.target, None);
        assert!(memory.timestamp_ms > 0);
    }

    #[test]
    fn state_round_trips_the_clamped_config() {
        let state = AlertState::default();
        assert_eq!(state.get(), AlertConfig::default());
        let applied = state.set(AlertConfig {
            consecutive: 999,
            ..Default::default()
        });
        assert_eq!(applied.consecutive, 60);
        assert_eq!(state.get(), applied, "命令返回的必须就是循环真正使用的值");
    }
}
