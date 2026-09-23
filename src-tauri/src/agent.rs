//! 诊断 Agent（T5-04 / T5-05 / T5-06）。
//!
//! 边界是 04 文档 S4 那一条：**Agent 只建议，不执行**。这条边界靠类型而不是靠约定守：
//! - 输入是自由文本，只用来"猜意图"，永远不会被拼成任何东西 —— [`Suggestion`] 里
//!   只有"打开哪个页签""定位哪个 PID"，没有命令、参数、路径字段可填。
//! - 只读 [`crate::monitor`] 已有的采集结果，不去读命令行参数、环境变量或其他用户的数据。
//! - 输出文本全部过 `log_sanitize`（进程名和挂载点里可能带用户目录）。
//! - 没有数据来源的字段返回 `None` / 空列表，绝不用 0 值凑数。

use crate::alert::{AlertConfig, AlertThresholds};
use crate::log_sanitize::sanitize;
use crate::monitor::{DiskMetrics, MetricsSnapshot, ProcessInfo};
use crate::platform::thermal::{Sensor, SensorSeverity, ThermalReport};
use serde::Serialize;

/// 查询回显与意图识别前的截断长度。按**字符**截，中文查询按字节切会落在多字节中间。
pub const MAX_QUERY_CHARS: usize = 200;
/// 排名类结论最多列几条。
pub const MAX_RANK: usize = 5;
const MAX_FINDINGS: usize = 8;
const MAX_SUGGESTIONS: usize = 4;
const MAX_NOTES: usize = 4;
/// 磁盘结论最多列几个分区，避免外挂一堆卷时把报告刷满。
const MAX_DISK_FINDINGS: usize = 4;
const MAX_NETWORK_FINDINGS: usize = 4;
/// 温度结论最多列几条（按读数从高到低）。
const MAX_THERMAL_FINDINGS: usize = 3;
/// 风扇最多列几条：转速不是"越高越危险"，列多了反而像在排行。
const MAX_FAN_FINDINGS: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Intent {
    /// "哪个进程占 CPU 最高"
    RankProcesses,
    /// "为什么这么卡" —— 综合 CPU / 内存 / 磁盘 / 进程出一份报告
    DiagnoseSlowness,
    /// "内存是不是快满了"
    MemoryPressure,
    /// "磁盘还剩多少"
    DiskSpace,
    /// "现在网速多少"
    NetworkThroughput,
    /// 启动项相关：Agent 不读启动项列表，只给导航建议
    StartupItems,
    /// 垃圾清理相关：Agent 不发起扫描，只给导航建议
    JunkFiles,
    /// 温度 / 风扇：读数来自 `platform::thermal`，没有免提权通路时如实说原因（T5-08）
    Temperature,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RankBy {
    Cpu,
    Memory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Severity {
    Info,
    Warning,
    Critical,
}

/// 结论归属的指标，界面据此配图标。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentMetric {
    Cpu,
    Memory,
    Disk,
    Network,
    Process,
    /// 温度 / 风扇（T5-08）。判定只用传感器自己上报的上限，不新造一套阈值。
    Thermal,
}

/// 预定义操作模板的全部种类：**只有导航与定位**。
/// 新增变体必须保持"不含命令/参数/路径"，`reply_serializes_no_executable_payload` 会挡住。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Suggestion {
    /// 切到某个页签。`tab` 只允许是 App.tsx 里那 8 个 key 之一。
    #[serde(rename_all = "camelCase")]
    OpenTab { tab: &'static str, label: String },
    /// 在进程表里按 PID 过滤定位。只填关键字，不代替用户点任何按钮。
    #[serde(rename_all = "camelCase")]
    FocusProcess {
        pid: u32,
        name: String,
        label: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub metric: AgentMetric,
    /// 人话结论，已脱敏
    pub text: String,
    /// 度量值；无来源时为 None，界面显示 `—`
    pub value: Option<String>,
    pub level: Severity,
}

/// 意图识别结果。`refused` 表示这句话看起来是在**要 Agent 去执行**什么，
/// 按 S4 一律拒绝，与"没听懂"区分开，界面才能给不同的文案。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Parsed {
    pub intent: Intent,
    pub rank_by: Option<RankBy>,
    pub refused: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reply {
    /// 原样回显（截断 + 脱敏），让用户看到 Agent 实际理解了哪句话
    pub query: String,
    pub intent: Intent,
    pub rank_by: Option<RankBy>,
    pub refused: bool,
    pub findings: Vec<Finding>,
    pub suggestions: Vec<Suggestion>,
    pub notes: Vec<String>,
    /// 恒为 false。私有且没有 setter —— 结构上就不存在"已执行"这个状态。
    executed: bool,
}

impl Reply {
    fn new(query: String, parsed: &Parsed) -> Self {
        Self {
            query,
            intent: parsed.intent,
            rank_by: parsed.rank_by,
            refused: parsed.refused,
            findings: Vec::new(),
            suggestions: Vec::new(),
            notes: Vec::new(),
            executed: false,
        }
    }

    /// 唯一读取"是否已执行"的入口。生产代码里没有调用方（前端只认序列化后的
    /// `executed: false`），留着是给边界测试断言用的 —— 所以显式允许 dead_code，
    /// 而不是把私有字段改成 pub 让谁都能写。
    #[allow(dead_code)]
    pub fn executed(&self) -> bool {
        self.executed
    }

    fn note(&mut self, text: impl Into<String>) {
        if self.notes.len() < MAX_NOTES {
            self.notes.push(text.into());
        }
    }

    fn find(&mut self, finding: Finding) {
        if self.findings.len() < MAX_FINDINGS {
            self.findings.push(finding);
        }
    }

    fn suggest(&mut self, suggestion: Suggestion) {
        if self.suggestions.len() < MAX_SUGGESTIONS
            && !self.suggestions.contains(&suggestion)
        {
            self.suggestions.push(suggestion);
        }
    }
}

/// Agent 能看到的**全部**数据：两个排好序的进程切片 + 一帧指标快照 + 当前阈值配置
/// + 一份温度/风扇报告（T5-08，由命令层采好再传进来，引擎自己不去碰文件系统）。
///
/// 刻意不含进程命令行、环境变量、用户目录。
#[derive(Debug, Clone, Copy)]
pub struct Input<'a> {
    pub snapshot: Option<&'a MetricsSnapshot>,
    /// 按 CPU 降序的切片（来自一次独立采集，不受进程表关键字影响）
    pub cpu_ranked: &'a [ProcessInfo],
    /// 按内存降序的切片
    pub mem_ranked: &'a [ProcessInfo],
    /// 复用告警引擎的阈值，避免"界面上 80 % 报警、Agent 却说这很正常"两套口径
    pub alert: &'a AlertConfig,
    /// 温度/风扇。空报告一定带着"为什么空"，所以这里不需要 `Option`
    pub thermal: &'a ThermalReport,
}

// ==================== 意图识别 ====================

fn has(hay: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| hay.contains(n))
}

/// 截断 + 去首尾空白：**保留大小写**，这是回显给用户看的那一份。
/// 之前直接拿 `normalize` 的结果回显，用户输入"哪个进程占 CPU 最高"会看到"cpu"。
fn clip(raw: &str) -> String {
    raw.chars()
        .take(MAX_QUERY_CHARS)
        .collect::<String>()
        .trim()
        .to_string()
}

/// `clip` + 转小写（英文关键字匹配用）。
fn normalize(raw: &str) -> String {
    clip(raw).to_lowercase()
}

const CPU_WORDS: &[&str] = &["cpu", "处理器", "占用率"];
const MEM_WORDS: &[&str] = &["内存", "memory", "ram", "运存"];
const PROCESS_WORDS: &[&str] = &["进程", "process", "程序", "pid"];
const RANK_WORDS: &[&str] = &["最高", "最大", "最多", "排行", "排名", "top", "谁", "哪个", "哪些", "占用"];
const DISK_WORDS: &[&str] = &["磁盘", "硬盘", "分区", "空间", "容量", "disk", "存储"];
const NET_WORDS: &[&str] = &["网络", "网速", "带宽", "上传", "下载", "网卡", "流量", "network"];
const STARTUP_WORDS: &[&str] = &["启动项", "开机", "自启", "startup", "login item"];
const JUNK_WORDS: &[&str] = &["垃圾", "缓存", "清理", "junk", "临时文件"];
const TEMP_WORDS: &[&str] = &["温度", "风扇", "过热", "thermal", "temperature"];
const SLOW_WORDS: &[&str] = &["卡", "慢", "诊断", "为什么", "怎么回事", "报告", "状况", "slow", "lag", "freeze"];
/// 看起来在要求"代执行"的自由文本。命中即拒绝，只给一句说明。
const COMMAND_LIKE: &[&str] = &[
    "sudo", "rm -", "chmod", "chown", "kill ", "killall", "pkill", "bash", "zsh -c",
    "sh -c", "curl", "wget", "osascript", "执行命令", "运行命令", "跑个命令", "执行脚本",
    "帮我关", "帮我杀", "终止进程", "结束进程", "杀掉", "强制退出", "关掉进程", "删掉进程",
    "shell", "命令行", "终端里",
];

/// 只做分类，不做任何"理解"以外的事情：认不出来就是 `Unknown`，
/// 认出来是命令请求就是 `Unknown + refused`。
pub fn parse(raw: &str) -> Parsed {
    let q = normalize(raw);
    if q.is_empty() {
        return Parsed { intent: Intent::Unknown, rank_by: None, refused: false };
    }
    if has(&q, COMMAND_LIKE) {
        return Parsed { intent: Intent::Unknown, rank_by: None, refused: true };
    }

    let rank_by = if has(&q, CPU_WORDS) {
        Some(RankBy::Cpu)
    } else if has(&q, MEM_WORDS) {
        Some(RankBy::Memory)
    } else {
        None
    };
    let about_process = has(&q, PROCESS_WORDS);
    let wants_rank = has(&q, RANK_WORDS);
    let rank = |by: RankBy| Parsed { intent: Intent::RankProcesses, rank_by: Some(by), refused: false };

    // 先判"我们根本没有数据"的三类，否则会被下面的综合诊断吃掉。
    if has(&q, TEMP_WORDS) {
        return Parsed { intent: Intent::Temperature, rank_by: None, refused: false };
    }
    if has(&q, STARTUP_WORDS) {
        return Parsed { intent: Intent::StartupItems, rank_by: None, refused: false };
    }
    if has(&q, JUNK_WORDS) {
        return Parsed { intent: Intent::JunkFiles, rank_by: None, refused: false };
    }
    // "占用最高的进程""谁最吃内存"：明确的排行榜
    if (about_process && (rank_by.is_some() || wants_rank)) || (wants_rank && rank_by.is_some()) {
        return rank(rank_by.unwrap_or(RankBy::Cpu));
    }
    if has(&q, DISK_WORDS) {
        return Parsed { intent: Intent::DiskSpace, rank_by: None, refused: false };
    }
    if has(&q, NET_WORDS) {
        return Parsed { intent: Intent::NetworkThroughput, rank_by: None, refused: false };
    }
    // 单独问内存（没有"排行"意味）：给压力判读而不是榜单
    if has(&q, MEM_WORDS) {
        return Parsed { intent: Intent::MemoryPressure, rank_by: None, refused: false };
    }
    if has(&q, SLOW_WORDS) || has(&q, CPU_WORDS) {
        return Parsed { intent: Intent::DiagnoseSlowness, rank_by: None, refused: false };
    }
    Parsed { intent: Intent::Unknown, rank_by: None, refused: false }
}

// ==================== 分析与生成 ====================

/// 入口：识别意图 → 在只读数据上判定 → 产出"待确认卡片"要渲染的内容。
/// 纯函数，不碰采集线程、不共享状态，因此 100 % 进 `cargo test --lib` 门禁。
pub fn answer(raw_query: &str, input: &Input) -> Reply {
    let parsed = parse(raw_query);
    let mut reply = Reply::new(sanitize(&clip(raw_query)), &parsed);

    match parsed.intent {
        Intent::RankProcesses => {
            let by = parsed.rank_by.unwrap_or(RankBy::Cpu);
            process_report(&mut reply, by, input);
            reply.suggest(Suggestion::OpenTab {
                tab: "processes",
                label: "打开进程表看完整排序".to_string(),
            });
        }
        Intent::DiagnoseSlowness => {
            metrics_report(&mut reply, input);
            process_report(&mut reply, RankBy::Cpu, input);
            process_report(&mut reply, RankBy::Memory, input);
            reply.suggest(Suggestion::OpenTab {
                tab: "processes",
                label: "打开进程表核对".to_string(),
            });
            reply.suggest(Suggestion::OpenTab {
                tab: "overview",
                label: "返回概览看趋势".to_string(),
            });
        }
        Intent::MemoryPressure => {
            if let Some(memory) = input.snapshot.map(|s| &s.memory) {
                let level = severity_of(memory.usage_percent, &input.alert.memory);
                reply.find(Finding {
                    metric: AgentMetric::Memory,
                    text: format!(
                        "已用 {} / {}，占 {:.1} %，压力等级：{}",
                        human_bytes(memory.used_bytes),
                        human_bytes(memory.total_bytes),
                        memory.usage_percent,
                        pressure_word(memory.pressure),
                    ),
                    value: Some(format!("{:.1} %", memory.usage_percent)),
                    level,
                });
                if memory.swap_total_bytes > 0 {
                    reply.find(Finding {
                        metric: AgentMetric::Memory,
                        text: format!(
                            "交换区已用 {} / {}",
                            human_bytes(memory.swap_used_bytes),
                            human_bytes(memory.swap_total_bytes),
                        ),
                        value: Some(format!(
                            "{:.0} %",
                            memory.swap_used_bytes as f64 / memory.swap_total_bytes as f64 * 100.0
                        )),
                        level: if memory.swap_used_bytes >= memory.swap_total_bytes / 2 {
                            Severity::Warning
                        } else {
                            Severity::Info
                        },
                    });
                }
            } else {
                no_source(&mut reply);
            }
            process_report(&mut reply, RankBy::Memory, input);
            reply.suggest(Suggestion::OpenTab {
                tab: "processes",
                label: "按内存排序看进程".to_string(),
            });
        }
        Intent::DiskSpace => {
            if input.snapshot.is_none() {
                no_source(&mut reply);
            } else {
                disk_report(&mut reply, input);
                reply.suggest(Suggestion::OpenTab {
                    tab: "disk",
                    label: "打开磁盘页签看各分区".to_string(),
                });
            }
        }
        Intent::NetworkThroughput => {
            network_report(&mut reply, input);
            reply.suggest(Suggestion::OpenTab {
                tab: "network",
                label: "打开网络页签看接口明细".to_string(),
            });
        }
        Intent::StartupItems => {
            reply.note(
                "Agent 不读取启动项列表：那是另一条采集链路，且禁用/删除启动项必须你自己在页面上确认。"
                    .to_string(),
            );
            reply.suggest(Suggestion::OpenTab {
                tab: "startup",
                label: "打开启动项页签".to_string(),
            });
        }
        Intent::JunkFiles => {
            reply.note(
                "Agent 不发起扫描、更不删除文件：扫描和清理都在清理页签里，由你逐项确认类别后执行。"
                    .to_string(),
            );
            reply.suggest(Suggestion::OpenTab {
                tab: "cleanup",
                label: "打开垃圾清理页签".to_string(),
            });
        }
        Intent::Temperature => {
            if input.thermal.sensors.is_empty() {
                // 没有通路就只说没有通路，连"大概几十度"这种话都不说。
                let reason = input
                    .thermal
                    .reason
                    .clone()
                    .unwrap_or_else(|| "温度采集没有返回任何说明。".to_string());
                reply.note(format!(
                    "{reason} 我不会用估算值、机型平均值或历史值代替读数。"
                ));
            } else {
                let mut temps: Vec<&Sensor> = input.thermal.temperatures().collect();
                temps.sort_by(|a, b| b.value.partial_cmp(&a.value).unwrap_or(std::cmp::Ordering::Equal));
                for sensor in temps.iter().take(MAX_THERMAL_FINDINGS) {
                    reply.find(thermal_finding(sensor));
                }
                for sensor in input.thermal.fans().take(MAX_FAN_FINDINGS) {
                    reply.find(fan_finding(sensor));
                }
                reply.suggest(Suggestion::OpenTab {
                    tab: "system",
                    label: format!(
                        "打开系统信息看全部 {} 个传感器",
                        input.thermal.sensors.len()
                    ),
                });
            }
        }
        Intent::Unknown => {
            if parsed.refused {
                reply.note(
                    "这条看起来是在要求我执行操作。Agent 只给建议、不执行任何命令，也不会生成命令行；\
                     终止进程、清理文件、刷新 DNS 都要在对应页面上由你确认后走各自的校验流程。"
                        .to_string(),
                );
            } else {
                reply.note("没有匹配到预定义的诊断模板，所以这条没有任何结论和建议。可以试试：哪个进程占 CPU 最高、为什么这么卡、磁盘还剩多少。".to_string());
            }
        }
    }

    reply
}

fn no_source(reply: &mut Reply) {
    reply.note("还没有可用的指标快照（采集第一帧未到），我不编造数值。".to_string());
}

/// 温度一条结论。等级直接取后端算好的 [`Sensor::severity`] —— 判级的唯一口径在 `thermal.rs`，
/// Agent 不再自己比一次大小，否则会出现"界面标黄、Agent 说正常"两套说法。
fn thermal_finding(sensor: &Sensor) -> Finding {
    let level = match sensor.severity {
        SensorSeverity::Critical => Severity::Critical,
        SensorSeverity::Warning => Severity::Warning,
        // unknown 只说明"这块传感器没上报上限"，不等于"温度正常"，所以文案里要显式讲出来
        SensorSeverity::Unknown | SensorSeverity::Ok => Severity::Info,
    };
    let text = match sensor.critical {
        Some(limit) => format!(
            "{} {:.1} {}，硬件上限 {:.0} {}",
            sensor.label,
            sensor.value,
            sensor.kind.unit(),
            limit,
            sensor.kind.unit()
        ),
        None => format!(
            "{} {:.1} {}（这块传感器没有上报自己的上限）",
            sensor.label,
            sensor.value,
            sensor.kind.unit()
        ),
    };
    Finding {
        metric: AgentMetric::Thermal,
        text,
        value: Some(format!("{:.1} {}", sensor.value, sensor.kind.unit())),
        level,
    }
}

/// 风扇一条结论。0 RPM 是"这一路没在转"，属于要单独说出来的读数，不能被当成缺测。
fn fan_finding(sensor: &Sensor) -> Finding {
    let stopped = sensor.severity == SensorSeverity::Warning;
    Finding {
        metric: AgentMetric::Thermal,
        text: if stopped {
            format!("{} 0 {}：这一路风扇当前停转（有读数，不是缺测）", sensor.label, sensor.kind.unit())
        } else {
            format!("{} {:.0} {}", sensor.label, sensor.value, sensor.kind.unit())
        },
        value: Some(format!("{:.0} {}", sensor.value, sensor.kind.unit())),
        level: if stopped { Severity::Warning } else { Severity::Info },
    }
}

fn severity_of(value: f64, thresholds: &AlertThresholds) -> Severity {
    if value >= thresholds.critical {
        Severity::Critical
    } else if value >= thresholds.warning {
        Severity::Warning
    } else {
        Severity::Info
    }
}

fn pressure_word(pressure: crate::monitor::MemoryPressure) -> &'static str {
    use crate::monitor::MemoryPressure::*;
    match pressure {
        Normal => "正常",
        Warning => "偏高",
        Critical => "紧张",
    }
}

fn metrics_report(reply: &mut Reply, input: &Input) {
    let Some(snapshot) = input.snapshot else {
        no_source(reply);
        return;
    };
    reply.find(Finding {
        metric: AgentMetric::Cpu,
        text: format!(
            "{} 个核心的总体占用 {:.1} %",
            snapshot.cpu.core_count, snapshot.cpu.total
        ),
        value: Some(format!("{:.1} %", snapshot.cpu.total)),
        level: severity_of(snapshot.cpu.total, &input.alert.cpu),
    });
    reply.find(Finding {
        metric: AgentMetric::Memory,
        text: format!(
            "内存已用 {} / {}，压力等级：{}",
            human_bytes(snapshot.memory.used_bytes),
            human_bytes(snapshot.memory.total_bytes),
            pressure_word(snapshot.memory.pressure),
        ),
        value: Some(format!("{:.1} %", snapshot.memory.usage_percent)),
        level: severity_of(snapshot.memory.usage_percent, &input.alert.memory),
    });
    disk_report(reply, input);
}

/// 只统计"真的读得到"的分区：`available == false` 或容量为 0 的条目一律跳过，
/// 与告警引擎同一口径（否则会把"读不到"说成"满了"）。
fn usable_disks(snapshot: &MetricsSnapshot) -> impl Iterator<Item = &DiskMetrics> {
    snapshot
        .disks
        .iter()
        .filter(|d| d.available && d.total_bytes > 0)
}

fn disk_report(reply: &mut Reply, input: &Input) {
    let Some(snapshot) = input.snapshot else { return };
    let mut listed = 0usize;
    let mut skipped = 0usize;
    for disk in usable_disks(snapshot) {
        if disk.usage_percent < input.alert.disk.warning {
            continue;
        }
        if listed >= MAX_DISK_FINDINGS {
            skipped += 1;
            continue;
        }
        listed += 1;
        reply.find(Finding {
            metric: AgentMetric::Disk,
            text: format!(
                "{} 剩余 {}，已用 {:.1} %",
                sanitize(&disk.mount_point),
                human_bytes(disk.available_bytes),
                disk.usage_percent,
            ),
            value: Some(format!("{:.1} %", disk.usage_percent)),
            level: severity_of(disk.usage_percent, &input.alert.disk),
        });
    }
    if listed == 0 {
        reply.note("没有分区越过告警阈值，磁盘这条线暂时不背锅。".to_string());
    }
    if skipped > 0 {
        reply.note(format!("另有 {skipped} 个越限分区未列出（最多展示 {MAX_DISK_FINDINGS} 个）。"));
    }
}

fn network_report(reply: &mut Reply, input: &Input) {
    let Some(snapshot) = input.snapshot else {
        no_source(reply);
        return;
    };
    let mut nets: Vec<_> = snapshot.networks.iter().collect();
    nets.sort_by(|a, b| {
        (b.rx_bytes_per_sec + b.tx_bytes_per_sec)
            .total_cmp(&(a.rx_bytes_per_sec + a.tx_bytes_per_sec))
    });
    if nets.is_empty() {
        reply.note("这一帧里没有网卡数据。".to_string());
        return;
    }
    for net in nets.iter().take(MAX_NETWORK_FINDINGS) {
        reply.find(Finding {
            metric: AgentMetric::Network,
            text: format!(
                "{} 收 {} · 发 {}",
                sanitize(&net.interface),
                human_rate(net.rx_bytes_per_sec),
                human_rate(net.tx_bytes_per_sec),
            ),
            value: Some(human_rate(net.rx_bytes_per_sec + net.tx_bytes_per_sec)),
            level: Severity::Info,
        });
    }
}

/// 排行榜：调用方给的是"已排序切片"，这里仍然重排一次并按键取值 ——
/// 不信任上游顺序，界面才不会出现"第一名比第二名低"的结论。
fn ranked(rows: &[ProcessInfo], by: RankBy, limit: usize) -> Vec<&ProcessInfo> {
    let mut picked: Vec<&ProcessInfo> = rows.iter().collect();
    let key = |p: &ProcessInfo| match by {
        RankBy::Cpu => p.cpu_usage,
        RankBy::Memory => p.memory_bytes as f64,
    };
    picked.sort_by(|a, b| key(b).total_cmp(&key(a)));
    picked.truncate(limit);
    picked
}

fn all_zero(rows: &[ProcessInfo], by: RankBy) -> bool {
    !rows.is_empty()
        && rows.iter().all(|p| match by {
            RankBy::Cpu => p.cpu_usage <= 0.0,
            RankBy::Memory => p.memory_bytes == 0,
        })
}

fn process_report(reply: &mut Reply, by: RankBy, input: &Input) {
    let rows = match by {
        RankBy::Cpu => input.cpu_ranked,
        RankBy::Memory => input.mem_ranked,
    };
    let unit = match by {
        RankBy::Cpu => "CPU",
        RankBy::Memory => "内存",
    };
    if rows.is_empty() {
        reply.note(format!("这一轮没有拿到进程枚举结果，{unit} 排行榜为空。"));
        return;
    }
    if all_zero(rows, by) {
        reply.note(format!("{unit} 采样还在暖机（两次采样的差值未到），这一轮不给出{unit}排行榜。"));
        return;
    }
    for (idx, proc) in ranked(rows, by, MAX_RANK).into_iter().enumerate() {
        let value = match by {
            RankBy::Cpu => format!("{:.1} %", proc.cpu_usage),
            RankBy::Memory => human_bytes(proc.memory_bytes),
        };
        reply.find(Finding {
            metric: AgentMetric::Process,
            text: format!(
                "#{} {}（PID {}）{unit} {value}",
                idx + 1,
                sanitize(&proc.name),
                proc.pid
            ),
            value: Some(value),
            level: Severity::Info,
        });
    }
    if let Some(top) = ranked(rows, by, 1).first() {
        reply.suggest(Suggestion::FocusProcess {
            pid: top.pid,
            name: sanitize(&top.name),
            label: match by {
                RankBy::Cpu => "在进程表里定位这个 CPU 大户",
                RankBy::Memory => "在进程表里定位这个内存大户",
            }
            .to_string(),
        });
    }
}

// ==================== 展示格式化 ====================

/// 与前端 `formatBytes` 同一口径：1024 进制、两位小数、B/KB/MB/GB/TB。
fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.2} {}", UNITS[unit])
}

fn human_rate(bytes_per_sec: f64) -> String {
    let rounded = if bytes_per_sec.is_finite() { bytes_per_sec.max(0.0).round() } else { 0.0 };
    format!("{}/s", human_bytes(rounded as u64))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::monitor::{CpuMetrics, MemoryMetrics, MemoryPressure, NetworkMetrics};
    use crate::platform::thermal::{self, SensorKind};

    fn proc(pid: u32, name: &str, cpu: f64, mem: u64) -> ProcessInfo {
        ProcessInfo {
            pid,
            name: name.to_string(),
            cpu_usage: cpu,
            memory_bytes: mem,
            threads: None,
            user_name: None,
            parent_pid: None,
            run_time_seconds: None,
        }
    }

    fn snapshot_with(cpu_total: f64, mem_used: u64, mem_total: u64) -> MetricsSnapshot {
        MetricsSnapshot {
            timestamp_ms: 1,
            uptime_seconds: 60,
            cpu: CpuMetrics { total: cpu_total, per_core: vec![cpu_total], core_count: 10 },
            memory: MemoryMetrics {
                total_bytes: mem_total,
                used_bytes: mem_used,
                available_bytes: mem_total - mem_used,
                swap_total_bytes: 0,
                swap_used_bytes: 0,
                usage_percent: mem_used as f64 / mem_total as f64 * 100.0,
                pressure: MemoryPressure::Normal,
            },
            disks: vec![],
            networks: vec![],
        }
    }

    fn disk(name: &str, mount: &str, total: u64, used: u64, available: bool) -> crate::monitor::DiskMetrics {
        crate::monitor::DiskMetrics {
            name: name.to_string(),
            mount_point: mount.to_string(),
            file_system: "apfs".to_string(),
            total_bytes: total,
            used_bytes: used,
            available_bytes: total - used,
            usage_percent: used as f64 / total as f64 * 100.0,
            read_bytes_per_sec: None,
            write_bytes_per_sec: None,
            available,
        }
    }

    struct Fixture {
        snapshot: MetricsSnapshot,
        cpu: Vec<ProcessInfo>,
        mem: Vec<ProcessInfo>,
        alert: AlertConfig,
        thermal: ThermalReport,
    }

    impl Default for Fixture {
        fn default() -> Self {
            Self {
                snapshot: snapshot_with(42.0, 24 * 1024u64.pow(3), 32 * 1024u64.pow(3)),
                cpu: vec![proc(11, "Chrome Helper", 30.0, 2 * 1024u64.pow(3)), proc(12, "WindowServer", 12.0, 1024u64.pow(3))],
                mem: vec![proc(11, "Chrome Helper", 30.0, 2 * 1024u64.pow(3)), proc(13, "idea", 1.0, 6 * 1024u64.pow(3))],
                alert: AlertConfig::default(),
                // 默认当作"这台机器给不出温度"，键写死 "macos" 而不是 `env::consts::OS`：
                // Linux CI 上 probe() 走的是有货那条腿，用当台的值会让下面的 SMC 断言莫名红。
                thermal: ThermalReport::unavailable(thermal::unsupported_reason("macos")),
            }
        }
    }

    /// 造一份"Linux 上真读到传感器"的报告，不用碰文件系统（解析链路已由 `thermal.rs` 的单测覆盖）。
    /// 走 `Sensor::new` 而不是结构体字面量：判级必须和读数一起算出来，测试也不给"手填 severity"的机会。
    fn thermal_report(rows: &[(&str, SensorKind, f64, Option<f64>)]) -> ThermalReport {
        ThermalReport {
            sensors: rows
                .iter()
                .map(|(label, kind, value, critical)| {
                    Sensor::new(
                        (*label).to_string(),
                        *kind,
                        *value,
                        *critical,
                        format!("/sys/class/hwmon/hwmon0/{}1_input", kind_prefix(*kind)),
                    )
                })
                .collect(),
            reason: None,
        }
    }

    fn kind_prefix(kind: SensorKind) -> &'static str {
        match kind {
            SensorKind::Temperature => "temp",
            SensorKind::Fan => "fan",
        }
    }

    fn normal_thermal() -> ThermalReport {
        thermal_report(&[
            ("cpu_thermal temp1", SensorKind::Temperature, 48.5, Some(95.0)),
            ("nvme temp1", SensorKind::Temperature, 71.0, Some(75.0)),
            ("CPU FAN", SensorKind::Fan, 2410.0, None),
        ])
    }

    impl Fixture {
        fn input(&self) -> Input<'_> {
            Input {
                snapshot: Some(&self.snapshot),
                cpu_ranked: &self.cpu,
                mem_ranked: &self.mem,
                alert: &self.alert,
                thermal: &self.thermal,
            }
        }
    }

    fn ask(fixture: &Fixture, query: &str) -> Reply {
        answer(query, &fixture.input())
    }

    // ---------- T5-04 意图识别 ----------

    #[test]
    fn recognizes_a_cpu_ranking_question() {
        let parsed = parse("哪个进程占 CPU 最高？");
        assert_eq!(parsed.intent, Intent::RankProcesses);
        assert_eq!(parsed.rank_by, Some(RankBy::Cpu));
        assert!(!parsed.refused);
    }

    #[test]
    fn recognizes_a_memory_ranking_question_in_english() {
        let parsed = parse("top memory consuming processes");
        assert_eq!(parsed.intent, Intent::RankProcesses);
        assert_eq!(parsed.rank_by, Some(RankBy::Memory));
    }

    #[test]
    fn recognizes_the_slowness_question_as_a_full_report() {
        assert_eq!(parse("为什么这么卡").intent, Intent::DiagnoseSlowness);
        assert_eq!(parse("帮我诊断一下电脑").intent, Intent::DiagnoseSlowness);
    }

    #[test]
    fn recognizes_the_other_templates() {
        assert_eq!(parse("磁盘还剩多少空间").intent, Intent::DiskSpace);
        assert_eq!(parse("现在网速怎么样").intent, Intent::NetworkThroughput);
        assert_eq!(parse("内存快满了吗").intent, Intent::MemoryPressure);
        assert_eq!(parse("有哪些开机自启动项").intent, Intent::StartupItems);
        assert_eq!(parse("清理一下垃圾文件").intent, Intent::JunkFiles);
        assert_eq!(parse("CPU 温度多少").intent, Intent::Temperature);
        assert_eq!(parse("今天天气如何").intent, Intent::Unknown);
    }

    #[test]
    fn free_text_command_requests_are_refused() {
        for raw in [
            "sudo rm -rf /",
            "帮我杀掉 Chrome 进程",
            "执行命令 df -h",
            "killall Dock",
        ] {
            let parsed = parse(raw);
            assert!(parsed.refused, "未拒绝：{raw}");
            assert_eq!(parsed.intent, Intent::Unknown, "拒绝时不该给出意图：{raw}");
        }
    }

    #[test]
    fn clipping_is_char_safe_and_case_insensitive() {
        let long = "卡".repeat(5000);
        let parsed = parse(&long);
        assert_eq!(parsed.intent, Intent::DiagnoseSlowness);
        let reply = {
            let fixture = Fixture::default();
            ask(&fixture, &long)
        };
        assert!(reply.query.chars().count() <= MAX_QUERY_CHARS);
        // 英文整句也认得；rank_by 只在排行榜意图下才有值，综合诊断不给假排序
        let parsed = parse("WHY IS MY CPU BUSY");
        assert_eq!(parsed.intent, Intent::DiagnoseSlowness);
        assert_eq!(parsed.rank_by, None);
    }

    // ---------- T5-04 分析 ----------

    #[test]
    fn ranking_report_ignores_upstream_order_and_caps_rows() {
        // 故意打乱：最高的放在最后，并塞满 20 条
        let fixture = Fixture {
            cpu: (0..20u32)
                .map(|i| proc(100 + i, &format!("p{i}"), i as f64, 1024))
                .collect(),
            ..Default::default()
        };
        let reply = ask(&fixture, "哪个进程 CPU 占用最高");
        assert_eq!(reply.intent, Intent::RankProcesses);
        assert_eq!(reply.rank_by, Some(RankBy::Cpu));
        let ranked_rows: Vec<_> = reply.findings.iter().filter(|f| f.metric == AgentMetric::Process).collect();
        assert_eq!(ranked_rows.len(), MAX_RANK, "排行榜应被截到 {MAX_RANK} 条");
        assert!(ranked_rows[0].text.contains("p19"), "第一名应是采集里最高的：{}", ranked_rows[0].text);
        assert!(ranked_rows[0].value.as_deref().unwrap().starts_with("19"));
    }

    #[test]
    fn warming_cpu_slice_is_reported_as_no_data_not_as_a_zero_winner() {
        let fixture = Fixture {
            cpu: vec![proc(1, "a", 0.0, 100), proc(2, "b", 0.0, 200)],
            ..Default::default()
        };
        let reply = ask(&fixture, "cpu 最高的进程");
        assert!(!reply.findings.iter().any(|f| f.metric == AgentMetric::Process));
        assert!(reply.notes.iter().any(|n| n.contains("暖机")), "{:?}", reply.notes);
    }

    #[test]
    fn missing_snapshot_produces_no_invented_values() {
        let fixture = Fixture::default();
        let input = Input {
            snapshot: None,
            cpu_ranked: &fixture.cpu,
            mem_ranked: &fixture.mem,
            alert: &fixture.alert,
            thermal: &fixture.thermal,
        };
        let reply = answer("为什么这么卡", &input);
        // 指标快照缺席时，CPU/内存/磁盘三类的结论一条都不能有（进程榜是另一个数据源，照常给）
        let invented: Vec<&Finding> = reply
            .findings
            .iter()
            .filter(|f| matches!(f.metric, AgentMetric::Cpu | AgentMetric::Memory | AgentMetric::Disk))
            .collect();
        assert!(invented.is_empty(), "无来源时不得有指标结论：{invented:?}");
        assert!(!reply.findings.is_empty(), "进程排行榜不该被连带清空");
        assert!(reply.notes.iter().any(|n| n.contains("不编造")), "{:?}", reply.notes);
    }

    #[test]
    fn thresholds_come_from_the_alert_config() {
        let mut fixture = Fixture::default();
        fixture.alert.cpu.warning = 10.0;
        fixture.alert.cpu.critical = 20.0;
        let reply = ask(&fixture, "为什么这么卡");
        let cpu = reply.findings.iter().find(|f| f.metric == AgentMetric::Cpu).expect("应有 CPU 结论");
        assert_eq!(cpu.level, Severity::Critical, "42 % 越过自定义 critical=20 %");
    }

    #[test]
    fn unavailable_and_zero_capacity_disks_are_skipped() {
        let mut fixture = Fixture::default();
        fixture.snapshot.disks = vec![
            disk("Data", "/System/Volumes/Data", 1000, 990, true),
            disk("Gone", "/Volumes/Gone", 1000, 999, false),
        ];
        let reply = ask(&fixture, "磁盘满了吗");
        let texts: Vec<&String> = reply.findings.iter().map(|f| &f.text).collect();
        assert_eq!(texts.len(), 1, "不可用分区不该进结论：{texts:?}");
        assert!(texts[0].contains("/System"));
    }

    #[test]
    fn mount_points_and_process_names_are_sanitized() {
        let mut fixture = Fixture::default();
        fixture.snapshot.disks = vec![disk("Data", "/Users/zifang/SecretDir", 1000, 990, true)];
        fixture.mem = vec![proc(7, "/Users/zifang/private_tool", 1.0, 6 * 1024u64.pow(3))];
        for query in ["磁盘满了吗", "内存占用最高的进程"] {
            let json = serde_json::to_string(&ask(&fixture, query)).unwrap();
            assert!(!json.contains("zifang"), "{query} 的回显里泄漏了用户名：{json}");
            assert!(json.contains("<user>"), "{query} 应看到脱敏占位：{json}");
        }
        // 排行榜第一名必须真的在结论里
        let ranked = ask(&fixture, "内存占用最高的进程");
        assert_eq!(ranked.findings.len(), 1, "{:?}", ranked.findings);
    }

    #[test]
    fn unimplemented_temperature_is_answered_honestly() {
        let fixture = Fixture::default();
        let reply = ask(&fixture, "CPU 温度多少");
        assert!(reply.findings.is_empty(), "没有通路时一条读数结论都不该有：{:?}", reply.findings);
        assert!(reply.suggestions.is_empty(), "没有可看的东西就不该推着用户去开页面");
        assert_eq!(reply.notes.len(), 1, "{:?}", reply.notes);
        // 原因来自 thermal，而不是 Agent 自己写死一句"未实现"
        assert!(reply.notes[0].contains("SMC"), "{:?}", reply.notes);
        assert!(reply.notes[0].contains("应用内不执行提权"), "{:?}", reply.notes);
        assert!(reply.notes[0].contains("不会用估算值"), "{:?}", reply.notes);
    }

    /// 有读数时：温度按从高到低排，等级只用传感器自己上报的上限。
    #[test]
    fn temperature_answer_ranks_real_sensors_against_their_own_limits() {
        let fixture = Fixture {
            thermal: normal_thermal(),
            ..Default::default()
        };
        let reply = ask(&fixture, "CPU 温度多少");
        assert_eq!(reply.intent, Intent::Temperature);
        let temps: Vec<&Finding> = reply
            .findings
            .iter()
            .filter(|f| f.text.contains("°C"))
            .collect();
        assert_eq!(temps.len(), 2, "{:?}", reply.findings);
        assert!(temps[0].text.starts_with("nvme"), "71 °C 该排在 48.5 °C 前面：{:?}", temps[0].text);
        assert_eq!(temps[0].level, Severity::Warning, "71/75 距上限不足 10 度");
        assert_eq!(temps[1].level, Severity::Info, "48.5/95 还很远");
        assert_eq!(temps[0].value.as_deref(), Some("71.0 °C"));

        let fan = reply.findings.iter().find(|f| f.text.contains("CPU FAN")).expect("应有风扇结论");
        assert_eq!(fan.level, Severity::Info);
        assert!(fan.text.contains("2410 RPM"), "{:?}", fan.text);

        assert!(reply.notes.is_empty(), "有读数时不该再挂\"没有通路\"的说明：{:?}", reply.notes);
        assert_eq!(
            reply.suggestions,
            vec![Suggestion::OpenTab {
                tab: "system",
                label: "打开系统信息看全部 3 个传感器".to_string(),
            }]
        );
    }

    /// 越过硬件上限要说"危险"，且这条判断的依据必须是硬件自己给的数，不是 Agent 猜的常量。
    #[test]
    fn a_sensor_past_its_own_limit_is_critical() {
        let fixture = Fixture {
            thermal: thermal_report(&[("GPU die", SensorKind::Temperature, 96.0, Some(95.0))]),
            ..Default::default()
        };
        let reply = ask(&fixture, "风扇转速");
        let finding = &reply.findings[0];
        assert_eq!(finding.level, Severity::Critical, "{:?}", finding);
        assert!(finding.text.contains("96.0") && finding.text.contains("95"), "{:?}", finding.text);
    }

    /// 传感器没上报自己的上限时，只报读数、不给等级 —— 免得把"不知道"说成"正常"。
    #[test]
    fn a_sensor_without_a_reported_limit_stays_info() {
        let fixture = Fixture {
            thermal: thermal_report(&[("acpitz", SensorKind::Temperature, 120.0, None)]),
            ..Default::default()
        };
        let reply = ask(&fixture, "温度");
        assert_eq!(reply.findings[0].level, Severity::Info);
        assert!(reply.findings[0].text.contains("没有上报自己的上限"), "{:?}", reply.findings[0].text);
    }

    /// 停转的风扇是 Warning，不是"没有数据"。
    #[test]
    fn a_fan_reading_zero_is_a_warning_not_a_missing_value() {
        let fixture = Fixture {
            thermal: thermal_report(&[("Case FAN", SensorKind::Fan, 0.0, None)]),
            ..Default::default()
        };
        let reply = ask(&fixture, "风扇");
        assert_eq!(reply.findings[0].level, Severity::Warning);
        assert!(reply.findings[0].text.contains("停转"), "{:?}", reply.findings[0].text);
        assert!(reply.findings[0].text.contains("不是缺测"), "{:?}", reply.findings[0].text);
        assert_eq!(reply.findings[0].value.as_deref(), Some("0 RPM"));
    }

    /// 温度条数要封顶：一块 GPU 导出 10 个热点时不该把报告刷满。
    #[test]
    fn temperature_findings_are_capped() {
        let rows: Vec<(&str, SensorKind, f64, Option<f64>)> = (0..10)
            .map(|i| ("hot", SensorKind::Temperature, 40.0 + i as f64, Some(95.0)))
            .collect();
        let fixture = Fixture {
            thermal: thermal_report(&rows),
            ..Default::default()
        };
        let reply = ask(&fixture, "温度");
        assert_eq!(
            reply.findings.iter().filter(|f| f.metric == AgentMetric::Thermal).count(),
            MAX_THERMAL_FINDINGS
        );
        assert!(reply.findings[0].text.contains("49.0"), "该列的是最高的那几个：{:?}", reply.findings[0].text);
    }

    /// 建议里那个页签必须真实存在（这条也在 `suggested_tabs_are_all_real_page_keys` 的覆盖面里）。
    #[test]
    fn the_temperature_suggestion_points_at_a_real_tab() {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let app = std::fs::read_to_string(manifest.join("../src/App.tsx")).unwrap();
        let fixture = Fixture { thermal: normal_thermal(), ..Default::default() };
        let reply = ask(&fixture, "温度");
        let Suggestion::OpenTab { tab, .. } = &reply.suggestions[0] else {
            panic!("温度建议只可能是打开页签：{:?}", reply.suggestions);
        };
        assert!(app.contains(&format!("key: \"{tab}\"")), "页签 {tab} 在 App.tsx 里不存在");
        // 系统信息页确实接了温度这条链路
        let tab_src = std::fs::read_to_string(manifest.join("../src/components/tabs/SystemInfoTab.tsx")).unwrap();
        assert!(tab_src.contains("thermal"), "系统信息页没有展示传感器");
    }

    #[test]
    fn unknown_intent_gives_nothing_to_act_on() {
        let fixture = Fixture::default();
        let reply = ask(&fixture, "讲个笑话");
        assert_eq!(reply.intent, Intent::Unknown);
        assert!(reply.findings.is_empty());
        assert!(reply.suggestions.is_empty());
        assert_eq!(reply.notes.len(), 1);
    }

    #[test]
    fn suggestions_and_findings_are_capped() {
        let mut fixture = Fixture::default();
        fixture.snapshot.disks = (0..10)
            .map(|i| disk(&format!("d{i}"), &format!("/mnt/d{i}"), 1000, 990, true))
            .collect();
        let reply = ask(&fixture, "为什么这么卡");
        assert!(reply.findings.len() <= MAX_FINDINGS, "{:?}", reply.findings.len());
        assert!(reply.suggestions.len() <= MAX_SUGGESTIONS, "{:?}", reply.suggestions);
        assert!(reply.notes.len() <= MAX_NOTES, "{:?}", reply.notes);
    }

    #[test]
    fn disk_findings_only_list_over_threshold() {
        let mut fixture = Fixture::default();
        fixture.snapshot.disks = vec![disk("Macintosh HD", "/", 1000, 400, true)];
        let reply = ask(&fixture, "磁盘空间");
        assert!(reply.findings.is_empty());
        assert!(reply.notes.iter().any(|n| n.contains("没有分区越过")), "{:?}", reply.notes);
    }

    #[test]
    fn network_report_ranks_interfaces_by_traffic() {
        use crate::monitor::InterfaceStatus;
        let mut fixture = Fixture::default();
        let net = |iface: &str, rx: f64, tx: f64| NetworkMetrics {
            interface: iface.to_string(),
            status: InterfaceStatus::Up,
            ipv4: None,
            ipv6: None,
            mac: None,
            rx_bytes_per_sec: rx,
            tx_bytes_per_sec: tx,
            total_received_bytes: 0,
            total_transmitted_bytes: 0,
            packets_received: 0,
            packets_transmitted: 0,
        };
        fixture.snapshot.networks = vec![net("lo0", 1.0, 1.0), net("en0", 5_000_000.0, 900_000.0)];
        let reply = ask(&fixture, "现在网速多少");
        assert!(reply.findings[0].text.contains("en0"), "{:?}", reply.findings);
        assert!(reply.findings[0].value.as_deref().unwrap().contains("MB"));
        // `human_rate` 自带 "/s"，模板里再写一次就成了 "KB/s/s"（真机 dump 里翻过车）。
        let text = &reply.findings[0].text;
        assert!(!text.contains("/s/s"), "速率后缀重复：{text}");
        assert_eq!(text.matches("/s").count(), 2, "两个速率各一个后缀：{text}");
    }

    /// 回显给用户的那一份必须保留原大小写：小写只用于关键字匹配。
    #[test]
    fn the_echo_keeps_the_users_own_capitalisation() {
        let fixture = Fixture::default();
        let reply = ask(&fixture, "哪个进程占 CPU 最高？");
        assert_eq!(reply.query, "哪个进程占 CPU 最高？", "{:?}", reply.query);
        // 匹配仍然走小写那一份，否则这条断言就成了空话
        assert_eq!(reply.intent, Intent::RankProcesses);
        assert_eq!(reply.rank_by, Some(RankBy::Cpu));
    }

    #[test]
    fn byte_formatting_matches_the_frontend() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(2048), "2.00 KB");
        assert_eq!(human_bytes(6 * 1024u64.pow(3)), "6.00 GB");
        assert_eq!(human_rate(3_500_000.0), "3.34 MB/s");
    }

    // ---------- T5-06 安全边界 ----------

    /// 递归收集 JSON 里所有对象的键名。
    fn keys(value: &serde_json::Value, out: &mut Vec<String>) {
        match value {
            serde_json::Value::Object(map) => {
                for (k, v) in map {
                    out.push(k.clone());
                    keys(v, out);
                }
            }
            serde_json::Value::Array(items) => items.iter().for_each(|v| keys(v, out)),
            _ => {}
        }
    }

    #[test]
    fn reply_serializes_no_executable_payload() {
        let fixture = Fixture::default();
        let queries = [
            "哪个进程占 CPU 最高", "为什么这么卡", "磁盘还剩多少", "网速", "内存压力",
            "启动项", "垃圾清理", "温度", "随便说点什么",
        ];
        let mut seen = Vec::new();
        for q in queries {
            let value = serde_json::to_value(answer(q, &fixture.input())).unwrap();
            keys(&value, &mut seen);
        }
        for forbidden in [
            "command", "argv", "args", "shell", "script", "program", "path", "exec",
            "executable", "cmd", "arguments", "token", "password", "env",
        ] {
            assert!(!seen.iter().any(|k| k == forbidden), "载荷里出现了可执行/敏感字段 {forbidden}：{seen:?}");
        }
        // 建议的种类被封死在两种导航模板里
        let kinds: Vec<String> = queries
            .iter()
            .flat_map(|q| {
                serde_json::to_value(answer(q, &fixture.input()))
                    .unwrap()["suggestions"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|s| s["kind"].as_str().unwrap_or_default().to_string())
            })
            .collect();
        for kind in &kinds {
            assert!(["openTab", "focusProcess"].contains(&kind.as_str()), "出现未知建议模板 {kind}");
        }
    }

    #[test]
    fn every_reply_is_marked_not_executed() {
        let fixture = Fixture::default();
        for q in ["为什么这么卡", "sudo rm -rf /", "哪个进程内存最高"] {
            let reply = ask(&fixture, q);
            assert!(!reply.executed());
            assert_eq!(serde_json::to_value(&reply).unwrap()["executed"], false);
        }
    }

    /// 源码审计：Agent 模块里不存在通往破坏性命令的调用点。
    /// `reply_serializes_no_executable_payload` 管住"说出口的内容"，这条管住"能开口的嘴"。
    /// 只扫 `#[cfg(test)]` 之前的生产代码 —— 下面这份黑名单本身就写着那些 token，
    /// 连测试一起扫等于自己绊自己。
    #[test]
    fn agent_module_has_no_destructive_call_path() {
        let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/agent.rs"))
            .expect("agent.rs 必须存在");
        let production = src.split("#[cfg(test)]").next().expect("应有测试模块");
        assert!(production.len() > 1000, "切分后生产代码段落过短，审计没有意义");
        for forbidden in [
            "kill_process(", "cleanup_junk_files(", "cancel_junk_scan(", "flush_dns_cache(",
            "Command::new", "std::process::", "std::fs::remove", "std::fs::write",
            "std::env::var", "invoke(", "spawn(",
        ] {
            assert!(!production.contains(forbidden), "Agent 里出现了执行通路 {forbidden}");
        }
    }

    #[test]
    fn agent_command_is_registered_and_typed_on_both_sides() {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let lib_rs = std::fs::read_to_string(manifest.join("src/lib.rs")).unwrap();
        assert!(lib_rs.contains("mod agent;"), "agent 模块没接进 lib.rs");
        assert!(lib_rs.contains("commands::agent_query"), "命令没注册进 invoke_handler");
        let commands_rs = std::fs::read_to_string(manifest.join("src/commands.rs")).unwrap();
        assert!(commands_rs.contains("agent::answer"), "commands.rs 未转调 Agent 引擎");
        let contract = std::fs::read_to_string(manifest.join("../src/ipc_contract.ts")).unwrap();
        assert!(contract.contains("agentQuery: \"agent_query\""), "前端未登记 agent_query");
    }

    /// SEC-V09 / A-15 / SEC-T08：Agent 面板必须零 IPC。
    /// 后端已经保证建议里只有"打开页签/定位进程"，这条再把"面板自己偷偷去调用命令"堵死 ——
    /// 面板拿到的只有回调，破坏性命令的 `invoke` 通路一条都不在同一个文件里。
    #[test]
    fn agent_panel_has_no_ipc_channel_of_its_own() {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let panel = manifest.join("../src/agent/AgentPanel.tsx");
        let src = std::fs::read_to_string(&panel).expect("AgentPanel.tsx 必须存在");
        for forbidden in [
            "invoke(", "@tauri-apps/api", "Commands.", "fetch(", "new Function",
            "dangerouslySetInnerHTML", "localStorage",
        ] {
            assert!(
                !src.contains(forbidden),
                "Agent 面板里出现了不该有的通路 {forbidden}（{}）",
                panel.display()
            );
        }
        // 面板唯一的动作出口就是那两个回调
        for required in ["onOpenTab", "onFocusProcess"] {
            assert!(src.contains(required), "面板缺了回调 {required}");
        }
    }

    #[test]
    fn suggested_tabs_are_all_real_page_keys() {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let app = std::fs::read_to_string(manifest.join("../src/App.tsx")).unwrap();
        let fixture = Fixture::default();
        for q in ["为什么这么卡", "磁盘", "网速", "内存", "启动项", "垃圾", "cpu 最高的进程"] {
            let reply = ask(&fixture, q);
            for suggestion in &reply.suggestions {
                if let Suggestion::OpenTab { tab, .. } = suggestion {
                    assert!(
                        app.contains(&format!("key: \"{tab}\"")),
                        "建议打开了一个不存在的页签 {tab}"
                    );
                }
            }
        }
    }
    /// R-07 的实测版。06 原来只写"默认本地模型；若接外部 API 需脱敏"，
    /// 而核对代码之后的事实更强：**本应用没有任何外呼通道**，Agent 是纯函数、也不可能有。
    /// 这条测试钉的就是这个"没有" —— 以后谁加了 HTTP 客户端依赖、在页面里写了 `fetch(`、
    /// 或者把 CSP 的 `connect-src` 放宽到外部站点，都会立刻红，而不是等出事才发现通道早就开了。
    #[test]
    fn r07_there_is_no_outbound_network_path_at_all() {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));

        let cargo_toml = std::fs::read_to_string(manifest.join("Cargo.toml")).unwrap();
        for outbound in [
            "reqwest",
            "hyper",
            "ureq",
            "curl",
            "attohttpc",
            "tauri-plugin-http",
            "tauri-plugin-opener",
        ] {
            assert!(
                !cargo_toml.contains(outbound),
                "Cargo.toml 里出现了外呼依赖 {outbound}：R-07「没有外呼通道」的前提不再成立"
            );
        }

        let raw = std::fs::read_to_string(manifest.join("tauri.conf.json")).unwrap();
        let conf: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let csp = conf["app"]["security"]["csp"]
            .as_str()
            .expect("CSP 必须是字符串（null 等于没有策略）");
        assert!(!csp.contains('*'), "CSP 里不许出现通配：{csp}");
        let connect = csp
            .split(';')
            .map(|part| part.trim())
            .find(|part| part.starts_with("connect-src"))
            .expect("CSP 必须有 connect-src");
        let mut saw_any = false;
        for target in connect.split_whitespace().skip(1) {
            saw_any = true;
            assert!(
                matches!(
                    target,
                    "ipc:" | "http://ipc.localhost" | "ws://localhost:1420" | "http://localhost:1420"
                ),
                "connect-src 出现了未预期目标 {target}：外呼通道被打开了"
            );
        }
        assert!(saw_any, "connect-src 是空的？那说明 CSP 结构变了，这条测试得跟着改");

        for file in ["../src/agent/AgentPanel.tsx", "../src/App.tsx", "../src/ipc_contract.ts"] {
            let text = std::fs::read_to_string(manifest.join(file)).unwrap();
            for dial in ["fetch(", "XMLHttpRequest", "new WebSocket", "EventSource"] {
                assert!(
                    !text.contains(dial),
                    "{file} 里出现了 {dial}：Agent 侧多了把系统信息带出本机的手法"
                );
            }
        }
    }
}
