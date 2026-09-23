//! 温度 / 风扇采集（T5-08）。
//!
//! 口径只有一条：**读到就说，读不到就说为什么读不到，中间不存在"估一个"**。
//! [`Sensor::value`] 因此在类型上就不是 `Option`，缺测的传感器根本不会进列表。
//!
//! 三条平台通路（判据见 `doc/优化方案/06` 的 T5-08 一节，都是本机实测不是推断）：
//!
//! - **Linux**：`/sys/class/hwmon/hwmon*` 下的 `tempN_input`（毫摄氏度）与 `fanN_input`（RPM）
//!   由内核导出，普通用户可读、不需要提权。`/sys/class/thermal/thermal_zone*` 只在 hwmon
//!   一条都没读到时兜底 —— 同一颗传感器常常两处都露出，优先 hwmon 才不会出重复行。
//! - **macOS**：读数在 SMC 后面。本机（Apple M3 Pro）实测过四条免提权通路，全部为空：
//!   `powermetrics --samplers smc` 回 `unrecognized sampler: smc`、`sysctl -a` 里没有任何温度键、
//!   `pmset -g therm` 三行全是 "No ... recorded"、IORegistry 只能 grep 到
//!   `AppleEmbeddedNVMeTemperatureSensor` 这类**类名与方法名**而没有数值。04 的口径是应用内不提权，
//!   所以这里返回原因，不去碰私有 SMC 接口。
//! - **Windows**：温度探针在 WMI（`Win32_TemperatureProbe` / `MSensor`）里，多数机型要管理员权限，
//!   而本项目的 Windows 腿从未真跑过一次采集（T5-09 的矩阵还没触发过第一次），所以同样只给原因。
//!
//! 结构上的取舍：解析与发现全部收在 [`probe_at`] 一个入口，sysfs 的根是**参数**不是硬编码 ——
//! 于是"只在 Linux 上成立的那套解析"在 macOS 上也能被单测真跑（测试注入一份假树），
//! 平台之间剩下的差别只有 [`root_free_source`] 那一行。

use crate::log_sanitize::sanitize;
use serde::Serialize;
use std::path::{Path, PathBuf};

/// 一次最多上报多少个传感器。有的驱动会把几十个空槽位一起导出，不封顶会把界面刷满。
pub const MAX_SENSORS: usize = 24;
/// Linux 下内核导出传感器的位置。写死在这里，唯一调用方就是 [`probe`]。
pub const SYSFS_ROOT: &str = "/sys";

/// 温度合理区间（°C）。hwmon 对"这个槽位没有接传感器"给的是 `i64::MIN` 一类的哨兵值，
/// 落在区间外的一律按"没有读数"处理，而不是报出"零下 21 亿度"。
const TEMP_RANGE: (f64, f64) = (-50.0, 150.0);
/// 风扇：**0 是真实读数**（停转），所以只有负数和物理上不可能的转速才丢。
const FAN_RANGE: (f64, f64) = (0.0, 30_000.0);
/// 距离硬件自己上报的上限不足这么多度，就算"快到了"。
/// 这是全项目**唯一**的温度判级依据：告警引擎那套百分比阈值管不到温度（它只有 CPU/内存/磁盘三类），
/// 所以界面与 Agent 都必须读 [`Sensor::severity`]，不得各自再算一遍 —— 否则就是"界面标黄、Agent 说正常"。
pub const WARN_HEADROOM_C: f64 = 10.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SensorKind {
    Temperature,
    Fan,
}

impl SensorKind {
    fn prefix(self) -> &'static str {
        match self {
            Self::Temperature => "temp",
            Self::Fan => "fan",
        }
    }

    pub fn unit(self) -> &'static str {
        match self {
            Self::Temperature => "°C",
            Self::Fan => "RPM",
        }
    }
}

/// 读数相对"硬件自己声明的上限"的位置。`unknown` 是一个**显式**状态：这块传感器没上报上限，
/// 于是界面不给颜色、Agent 也不说"正常" —— 把"不知道"渲染成绿色就是编数据。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SensorSeverity {
    Ok,
    Warning,
    Critical,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Sensor {
    /// 传感器自己的名字（sysfs 的 `tempN_label`，没有就退成"芯片名 + 槽位"，例如 `nvme temp1`）。
    pub label: String,
    pub kind: SensorKind,
    /// 真实读数，不是 `Option`：读不出来的槽位会被丢掉，界面上不存在"把 0 当成缺测"。
    pub value: f64,
    /// 传感器**自己声明**的临界值（hwmon 的 `tempN_max`）。
    pub critical: Option<f64>,
    /// 由 [`severity_of`] 在构造时算出，生产代码里没有第二处判级。
    pub severity: SensorSeverity,
    /// 读数出自哪个内核节点，界面上是"这块数据从哪来"的凭据。
    pub source: String,
}

impl Sensor {
    /// 唯一的构造入口：判级随读数一起算出来，不给调用方"先建一个再自己填 severity"的机会。
    pub(crate) fn new(
        label: String,
        kind: SensorKind,
        value: f64,
        critical: Option<f64>,
        source: String,
    ) -> Self {
        Self {
            label,
            kind,
            value,
            critical,
            severity: severity_of(kind, value, critical),
            source,
        }
    }
}

/// 温度：越上限 critical，距上限 `WARN_HEADROOM_C` 以内 warning，没有上限就 unknown。
/// 风扇：0 转就是 warning（停转是故障不是缺测），有转速就 ok；风扇没有"上限"可言。
pub fn severity_of(kind: SensorKind, value: f64, critical: Option<f64>) -> SensorSeverity {
    match kind {
        SensorKind::Temperature => match critical {
            Some(limit) if value >= limit => SensorSeverity::Critical,
            Some(limit) if value >= limit - WARN_HEADROOM_C => SensorSeverity::Warning,
            Some(_) => SensorSeverity::Ok,
            None => SensorSeverity::Unknown,
        },
        SensorKind::Fan => {
            if value <= 0.0 {
                SensorSeverity::Warning
            } else {
                SensorSeverity::Ok
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThermalReport {
    pub sensors: Vec<Sensor>,
    /// 为什么一条读数都没有。有读数时恒为 `None`，没有读数时恒不为空 ——
    /// [`an_empty_report_always_explains_itself`] 就是钉这条不变量的。
    pub reason: Option<String>,
}

impl ThermalReport {
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            sensors: Vec::new(),
            reason: Some(reason.into()),
        }
    }

    pub fn temperatures(&self) -> impl Iterator<Item = &Sensor> {
        self.sensors
            .iter()
            .filter(|s| s.kind == SensorKind::Temperature)
    }

    pub fn fans(&self) -> impl Iterator<Item = &Sensor> {
        self.sensors.iter().filter(|s| s.kind == SensorKind::Fan)
    }
}

/// 这个平台有没有"不提权就能读到传感器"的通路。
/// 按字符串判而不是 `cfg!`，测试才能在 macOS 上把三条分支都跑到。
pub fn root_free_source(os: &str) -> Option<&'static str> {
    match os {
        "linux" => Some(SYSFS_ROOT),
        _ => None,
    }
}

/// 没有通路时怎么说。每一句都要说清"为什么没有"，不能只留一个空列表让人以为是坏了。
///
/// 键必须是 `std::env::consts::OS` 真会给出的字符串：苹果上是 `"macos"` 而不是 `"darwin"`
/// （写错不会编译失败，只会静默落到兜底文案，等于每台 Mac 都拿到一句没有信息量的"还没实现"）。
/// [`the_apple_arm_is_keyed_on_the_string_rust_actually_reports`] 就是钉这一条的。
pub fn unsupported_reason(os: &str) -> String {
    match os {
        "macos" => "macOS 的温度与风扇读数在 SMC 里，取它要提权（`powermetrics` 需 root，且新版 macOS 的 smc sampler 不保证还在）；本应用内不执行提权，所以这一项没有读数。".to_string(),
        "windows" => "Windows 的温度探针在 WMI（`Win32_TemperatureProbe` / `MSensor`）里，多数机型上要管理员权限；且本项目从未在 Windows 上真跑过一次采集，所以这里不猜数值。".to_string(),
        other => format!("{other} 上还没有实现免提权的温度/风扇读取通路，所以没有读数。"),
    }
}

/// 生产入口：平台给得出 root 就真去读，给不出就带着原因返回。
pub fn probe() -> ThermalReport {
    match root_free_source(std::env::consts::OS) {
        Some(root) => probe_at(Path::new(root)),
        None => ThermalReport::unavailable(unsupported_reason(std::env::consts::OS)),
    }
}

/// 从一棵 sysfs 树里读传感器。root 是参数，所以这条链路可以在任何平台上被单测真跑。
pub fn probe_at(root: &Path) -> ThermalReport {
    let hwmon = discover_hwmon(&root.join("class/hwmon"));
    let found = if hwmon.is_empty() {
        // thermal_zone 与 hwmon 常常重复露出同一颗传感器，只在 hwmon 全空时才用它。
        discover_thermal_zones(&root.join("class/thermal"))
    } else {
        hwmon
    };
    if found.is_empty() {
        return ThermalReport::unavailable(format!(
            "{} 之下没有给出可读的温度/风扇节点（查的是 class/hwmon 与 class/thermal；虚拟机与容器里这很正常）",
            sanitize(&root.display().to_string())
        ));
    }
    let sensors = ranked_sensors(found);
    ThermalReport {
        sensors,
        reason: None,
    }
}

/// 带上排序键的中间态：`read_dir` 的顺序**不保证**，不排序就会出现每次刷新换个位置。
struct Located {
    /// (芯片目录的序号, 指标种类, 槽位号) —— 三级键，同芯片的多个传感器会挨在一起。
    key: (u32, u8, u32),
    sensor: Sensor,
}

fn ranked_sensors(mut found: Vec<Located>) -> Vec<Sensor> {
    found.sort_by_key(|l| l.key);
    found.truncate(MAX_SENSORS);
    found.into_iter().map(|l| l.sensor).collect()
}

fn discover_hwmon(dir: &Path) -> Vec<Located> {
    let mut out = Vec::new();
    for chip in sub_dirs(dir) {
        let chip_name = file_name(&chip);
        let ordinal = dir_ordinal(&chip_name);
        let chip_label = read_node(&chip.join("name")).unwrap_or_else(|| chip_name.clone());
        let mut slots: Vec<(u32, SensorKind)> = Vec::new();
        for node in sub_files(&chip) {
            let name = file_name(&node);
            if let Some((kind, index)) = sensor_slot(&name) {
                slots.push((index, kind));
            }
        }
        slots.sort();
        for (index, kind) in slots {
            if let Some(sensor) = hwmon_sensor(&chip, index, kind, &chip_label) {
                out.push(Located {
                    key: (ordinal, kind_rank(kind), index),
                    sensor,
                });
            }
        }
    }
    out
}

/// `temp12_input` → `(Temperature, 12)`。前缀与序号必须同时匹配，
/// 否则会把 `temp1_max`、`in0_input`、`power1_input` 这些也当成温度/风扇。
fn sensor_slot(name: &str) -> Option<(SensorKind, u32)> {
    let stem = name.strip_suffix("_input")?;
    let split = stem.find(|c: char| !c.is_ascii_alphabetic())?;
    let index: u32 = stem[split..].parse().ok()?;
    let kind = match &stem[..split] {
        "temp" => SensorKind::Temperature,
        "fan" => SensorKind::Fan,
        _ => return None,
    };
    Some((kind, index))
}

fn hwmon_sensor(chip: &Path, index: u32, kind: SensorKind, chip_label: &str) -> Option<Sensor> {
    let input = chip.join(format!("{}{}_input", kind.prefix(), index));
    let value = read_node(&input).and_then(|raw| scaled_reading(&raw, kind))?;
    let label = read_node(&chip.join(format!("{}{}_label", kind.prefix(), index)))
        .unwrap_or_else(|| format!("{chip_label} {}{}", kind.prefix(), index));
    let critical = match kind {
        SensorKind::Temperature => read_node(&chip.join(format!("temp{index}_max")))
            .and_then(|raw| scaled_reading(&raw, kind)),
        // 风扇的 `fanN_min` 是"转太低"，与温度的"临界上限"不是一个语义，不塞进同一个字段。
        SensorKind::Fan => None,
    };
    Some(Sensor::new(
        sanitize(&label),
        kind,
        value,
        critical,
        sanitize(&input.display().to_string()),
    ))
}

fn discover_thermal_zones(dir: &Path) -> Vec<Located> {
    let mut out = Vec::new();
    for zone in sub_dirs(dir) {
        let name = file_name(&zone);
        let ordinal = dir_ordinal(&name);
        let Some(value) = read_node(&zone.join("temp")).and_then(|raw| scaled_reading(&raw, SensorKind::Temperature))
        else {
            continue;
        };
        let label = read_node(&zone.join("type")).unwrap_or_else(|| name.clone());
        out.push(Located {
            key: (ordinal, kind_rank(SensorKind::Temperature), 0),
            sensor: Sensor::new(
                sanitize(&label),
                SensorKind::Temperature,
                value,
                None,
                sanitize(&zone.join("temp").display().to_string()),
            ),
        });
    }
    out
}

/// sysfs 的约定：温度是**毫**摄氏度，风扇已经是 RPM。区间外的（含"槽位空置"的哨兵值）丢掉。
fn scaled_reading(raw: &str, kind: SensorKind) -> Option<f64> {
    let milli: i64 = raw.trim().parse().ok()?;
    let value = match kind {
        SensorKind::Temperature => milli as f64 / 1000.0,
        SensorKind::Fan => milli as f64,
    };
    let (lo, hi) = match kind {
        SensorKind::Temperature => TEMP_RANGE,
        SensorKind::Fan => FAN_RANGE,
    };
    (value >= lo && value <= hi).then_some(value)
}

fn kind_rank(kind: SensorKind) -> u8 {
    match kind {
        SensorKind::Temperature => 0,
        SensorKind::Fan => 1,
    }
}

/// `hwmon3` / `thermal_zone10` → 3 / 10。按数字排，字典序会让 `hwmon10` 插到 `hwmon2` 前面。
fn dir_ordinal(name: &str) -> u32 {
    match name.rfind(|c: char| !c.is_ascii_digit()) {
        Some(i) => name[i + 1..].parse().unwrap_or(0),
        None => 0,
    }
}

fn read_node(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let trimmed = text.trim().to_string();
    (!trimmed.is_empty()).then_some(trimmed)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_string()
}

fn entries_of(dir: &Path) -> Vec<PathBuf> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = read.filter_map(|e| e.ok().map(|e| e.path())).collect();
    out.sort();
    out
}

fn sub_dirs(dir: &Path) -> Vec<PathBuf> {
    entries_of(dir).into_iter().filter(|p| p.is_dir()).collect()
}

fn sub_files(dir: &Path) -> Vec<PathBuf> {
    entries_of(dir).into_iter().filter(|p| p.is_file()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一棵假的 sysfs 树。放在临时目录里，所以这套断言在 macOS 上也是真的走了一遍文件 IO。
    struct Tree {
        root: PathBuf,
    }

    impl Tree {
        fn new(tag: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "zsys-thermal-{tag}-{}-{}",
                std::process::id(),
                SensorTestCell::next()
            ));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).expect("临时目录应可建");
            Self { root }
        }

        /// `hwmonN/<file> = <content>`
        fn hwmon(&self, chip_dir: &str, nodes: &[(&str, &str)]) -> &Self {
            let dir = self.root.join("class/hwmon").join(chip_dir);
            std::fs::create_dir_all(&dir).expect("hwmon 目录应可建");
            for (file, content) in nodes {
                std::fs::write(dir.join(file), format!("{content}\n")).expect("节点应可写");
            }
            self
        }

        fn thermal_zone(&self, zone_dir: &str, kind: &str, temp_milli: &str) -> &Self {
            let dir = self.root.join("class/thermal").join(zone_dir);
            std::fs::create_dir_all(&dir).expect("thermal 目录应可建");
            std::fs::write(dir.join("type"), format!("{kind}\n")).unwrap();
            std::fs::write(dir.join("temp"), format!("{temp_milli}\n")).unwrap();
            self
        }

        fn sensor(&self, report: &ThermalReport, label: &str) -> Sensor {
            report
                .sensors
                .iter()
                .find(|s| s.label == label)
                .unwrap_or_else(|| panic!("报告里没有 {label}：{:?}", report.sensors))
                .clone()
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    /// 同一进程内也要拿到不同的临时目录名（多个测试并发跑）。
    struct SensorTestCell;

    impl SensorTestCell {
        fn next() -> u32 {
            static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        }
    }

    #[test]
    fn hwmon_readings_are_scaled_from_millidegrees_and_keep_their_own_limit() {
        let tree = Tree::new("scale");
        tree.hwmon(
            "hwmon0",
            &[
                ("name", "cpu_thermal"),
                ("temp1_input", "52340"),
                ("temp1_label", "CPU0"),
                ("temp1_max", "95000"),
                ("fan1_input", "2410"),
                ("fan1_label", "CPU FAN"),
            ],
        );

        let report = probe_at(&tree.root);
        assert_eq!(report.sensors.len(), 2, "{:?}", report.sensors);
        assert!(report.reason.is_none());

        let temp = tree.sensor(&report, "CPU0");
        assert!((temp.value - 52.34).abs() < 1e-9, "毫摄氏度要除以 1000：{}", temp.value);
        assert_eq!(temp.critical, Some(95.0), "传感器自己声明的上限要带出去");
        assert_eq!(temp.kind, SensorKind::Temperature);
        // Windows 上 std 的 read_dir 会把短路径名展成对应的长路径（runner 的 TEMP 环境变量写作
        // `C:\Users\RUNNER~1\...`，目录实际叫 `...\runneradmin\...`），`source` 里 root 那一段的
        // 字面拼写因此由 OS 决定，不是产品能选的。这条断言要钉的是"读的是哪个节点"：
        // 按路径组件比尾部，再单独确认它落在本测试这棵唯一命名的临时树里。
        let node = Path::new("class").join("hwmon").join("hwmon0").join("temp1_input");
        assert!(
            Path::new(&temp.source).ends_with(&node),
            "读数要指回它真正来自的节点，实际：{}",
            temp.source
        );
        let tree_name = tree.root.file_name().unwrap().to_string_lossy().to_string();
        assert!(
            temp.source.contains(&tree_name),
            "读数不该指到别的临时树：{}",
            temp.source
        );

        let fan = tree.sensor(&report, "CPU FAN");
        assert_eq!(fan.value, 2410.0, "风扇已经是 RPM，不能再除 1000");
        assert_eq!(fan.kind, SensorKind::Fan);
        assert_eq!(fan.critical, None, "`fanN_min` 不是\"临界上限\"，不该塞进同一个字段");
    }

    /// 芯片没给 `tempN_label` 时退到芯片名 + 槽位号，而不是留空字符串（界面会显示成一行没有名字的数据）。
    #[test]
    fn a_sensor_without_its_own_label_falls_back_to_the_chip_name() {
        let tree = Tree::new("nolabel");
        tree.hwmon("hwmon2", &[("name", "nvme"), ("temp1_input", "44000")]);
        let report = probe_at(&tree.root);
        assert_eq!(report.sensors.len(), 1);
        assert_eq!(report.sensors[0].label, "nvme temp1");
    }

    #[test]
    fn absent_slots_never_become_a_zero_reading() {
        let tree = Tree::new("absent");
        tree.hwmon(
            "hwmon0",
            &[
                ("name", "coretemp"),
                // hwmon 对"没接传感器"的哨兵值
                ("temp1_input", "-2147483648"),
                ("temp2_input", "not a number"),
                ("temp3_input", "   "),
                // 超出物理合理区间（200 °C）
                ("temp4_input", "200000"),
                // 唯一真实的一个
                ("temp5_input", "45000"),
                ("fan2_input", "-1"),
            ],
        );

        let report = probe_at(&tree.root);
        assert_eq!(report.sensors.len(), 1, "四个坏槽位都不该变成一行：{:?}", report.sensors);
        assert!((report.sensors[0].value - 45.0).abs() < 1e-9);
        assert_eq!(report.sensors[0].label, "coretemp temp5");
    }

    /// 停转的风扇是**故障**不是缺测，0 必须如实报出来；负数与离谱值才丢。
    #[test]
    fn a_stopped_fan_is_a_real_zero_but_an_absurd_one_is_dropped() {
        let tree = Tree::new("fan");
        tree.hwmon(
            "hwmon0",
            &[
                ("name", "it87"),
                ("fan1_input", "0"),
                ("fan2_input", "999999"),
                ("fan3_input", "30000"),
            ],
        );
        let report = probe_at(&tree.root);
        let values: Vec<f64> = report.fans().map(|s| s.value).collect();
        assert_eq!(values, vec![0.0, 30_000.0], "{:?}", values);
        assert_eq!(report.temperatures().count(), 0, "没有温度节点就不该有温度行");
    }

    /// `read_dir` 的顺序不保证：排两次必须一模一样，且同芯片内按温度→风扇、槽位号升序。
    #[test]
    fn sensor_order_is_stable_even_though_readdir_is_not() {
        let tree = Tree::new("order");
        tree.hwmon("hwmon10", &[("name", "btt"), ("fan1_input", "800"), ("temp2_input", "40000")]);
        tree.hwmon("hwmon2", &[("name", "atc"), ("temp10_input", "60000"), ("temp1_input", "50000")]);

        let first = probe_at(&tree.root);
        let second = probe_at(&tree.root);
        assert_eq!(first, second, "两次采集的顺序不该变");

        let labels: Vec<String> = first.sensors.iter().map(|s| s.label.clone()).collect();
        assert_eq!(
            labels,
            vec!["atc temp1", "atc temp10", "btt temp2", "btt fan1"],
            "hwmon2 该排在 hwmon10 前，同芯片里温度该排在风扇前、槽位号按数字升序"
        );
    }

    /// hwmon 有货时不该再把 thermal_zone 的同一颗传感器重复列一遍。
    #[test]
    fn thermal_zones_only_fill_in_when_hwmon_has_nothing() {
        let tree = Tree::new("fallback");
        tree.hwmon("hwmon0", &[("name", "cpu_thermal"), ("temp1_input", "50000")]);
        tree.thermal_zone("thermal_zone0", "cpu-thermal", "50000");
        tree.thermal_zone("thermal_zone1", "acpitz", "47000");

        let report = probe_at(&tree.root);
        assert_eq!(report.sensors.len(), 1, "hwmon 有货时不该混进 thermal_zone：{:?}", report.sensors);

        let only_zones = Tree::new("fallback2");
        only_zones.thermal_zone("thermal_zone0", "x86_pkg_temp", "66500");
        let zones = probe_at(&only_zones.root);
        assert_eq!(zones.sensors.len(), 1);
        assert_eq!(zones.sensors[0].label, "x86_pkg_temp");
        assert!((zones.sensors[0].value - 66.5).abs() < 1e-9);
        assert_eq!(zones.sensors[0].critical, None, "thermal_zone 不上报自己的上限");
    }

    /// 有的驱动导出几十个空槽位，条数要封顶（且封顶是**先排序后截断**，截的是稳定的尾部）。
    #[test]
    fn sensor_rows_are_capped_and_the_cap_is_deterministic() {
        let tree = Tree::new("cap");
        let mut nodes: Vec<(String, String)> = vec![("name".to_string(), "many".to_string())];
        for i in 1..=40 {
            nodes.push((format!("temp{i}_input"), format!("{}", 30_000 + i)));
        }
        let refs: Vec<(&str, &str)> = nodes.iter().map(|(f, v)| (f.as_str(), v.as_str())).collect();
        tree.hwmon("hwmon0", &refs);

        let report = probe_at(&tree.root);
        assert_eq!(report.sensors.len(), MAX_SENSORS, "该正好封顶");
        assert_eq!(report.sensors.first().unwrap().label, "many temp1");
        assert_eq!(report.sensors.last().unwrap().label, "many temp24");
    }

    /// 目录不存在 / 是空目录，都要给出原因，而不是一个看起来像"坏了"的空报告。
    #[test]
    fn a_missing_tree_is_reported_as_no_source_not_as_an_error() {
        let report = probe_at(Path::new("/nonexistent-zsys-thermal-root"));
        assert!(report.sensors.is_empty());
        let reason = report.reason.unwrap();
        assert!(reason.contains("没有给出可读"), "{reason}");
        assert!(reason.contains("/nonexistent-zsys-thermal-root"), "原因里要带上查的是哪个根：{reason}");

        let empty = Tree::new("empty");
        let report = probe_at(&empty.root);
        assert!(report.sensors.is_empty());
        let reason = report.reason.unwrap();
        assert!(reason.contains("class"), "原因里该说到查过 hwmon/thermal：{reason}");
    }

    /// 芯片名与 label 来自内核，仍按 04 的口径过一遍脱敏（用户目录名不该从传感器标签里漏出去）。
    #[test]
    fn labels_and_sources_go_through_the_sanitizer() {
        let tree = Tree::new("sanitize");
        tree.hwmon(
            "hwmon0",
            &[
                ("name", "chip"),
                ("temp1_input", "50000"),
                ("temp1_label", "die /Users/zifang/private"),
            ],
        );
        let report = probe_at(&tree.root);
        let label = &report.sensors[0].label;
        assert!(!label.contains("zifang"), "标签里漏了用户名：{label}");
        assert!(label.contains("<user>"), "应被脱敏成 <user>：{label}");
    }

    /// 只认 `tempN_input` / `fanN_input`。`in0_input`（电压）、`power1_input`（功率）、
    /// `temp1_max`（不是读数）都不能进来 —— 否则电压会被当成温度显示成 °C。
    #[test]
    fn only_temperature_and_fan_slots_are_recognised() {
        assert_eq!(sensor_slot("temp12_input"), Some((SensorKind::Temperature, 12)));
        assert_eq!(sensor_slot("fan3_input"), Some((SensorKind::Fan, 3)));
        for name in [
            "temp1_max", "temp1_label", "in0_input", "power1_input", "temp_input", "tempX_input",
            "temp1_input_hyst", "crit_temp_input",
        ] {
            assert_eq!(sensor_slot(name), None, "{name} 不该被认成读数节点");
        }
    }

    #[test]
    fn chip_directories_are_ordered_numerically_not_lexically() {
        assert_eq!(dir_ordinal("hwmon2"), 2);
        assert_eq!(dir_ordinal("hwmon10"), 10);
        assert_eq!(dir_ordinal("thermal_zone7"), 7);
        assert_eq!(dir_ordinal("hwmon"), 0);
        // 这条是 `sensor_order_is_stable…` 的前提：字典序会把 hwmon10 排到 hwmon2 前面。
        assert!(dir_ordinal("hwmon2") < dir_ordinal("hwmon10"));
    }

    /// 免提权通路只给 Linux，且每条分支都要说清原因（这三条在 macOS 上就能全跑到）。
    #[test]
    fn the_only_root_free_source_is_linux_sysfs() {
        assert_eq!(root_free_source("linux"), Some(SYSFS_ROOT));
        assert_eq!(SYSFS_ROOT, "/sys", "root 一旦改动，下面所有 sysfs 路径的说明都要跟着改");
        for os in ["linux", "macos", "windows", "freebsd"] {
            let reason = unsupported_reason(os);
            assert!(!reason.is_empty(), "{os} 没有原因可给？");
        }
        assert!(unsupported_reason("macos").contains("SMC"));
        assert!(unsupported_reason("macos").contains("提权"));
        assert!(unsupported_reason("windows").contains("WMI"));
        assert!(unsupported_reason("freebsd").contains("freebsd"));
    }

    /// `unsupported_reason` 的键必须逐字等于 `std::env::consts::OS` 会给出的字符串。
    /// 本轮实测就是这么抓到 bug 的：苹果上 Rust 报 `"macos"`，写成 `"darwin"` 时那条专门文案
    /// 永远不会出现，每台 Mac 只拿到兜底的"还没有实现"，而单元测试因为自己喂了 `"darwin"` 全绿。
    #[test]
    fn the_apple_arm_is_keyed_on_the_string_rust_actually_reports() {
        if cfg!(target_os = "macos") {
            let reason = probe().reason.expect("macOS 没有免提权通路，空报告必须带着原因");
            assert!(reason.contains("SMC"), "macOS 上落到了兜底文案：{reason}");
            assert!(reason.contains("应用内不执行提权"), "要说清这里不提权：{reason}");
        }
        // 兜底句只该留给 Rust 真会报出的其它系统名
        assert!(unsupported_reason("freebsd").contains("freebsd"));
        assert!(unsupported_reason("freebsd").contains("还没有实现"));
    }

    /// 没有免提权通路的平台上，`probe()` 不得凭空产出一行读数。
    /// Linux CI 上这条自然恒真（root_free_source 给得出 root），关键是它**不会**在 Linux 上被跳过。
    #[test]
    fn probe_invents_nothing_on_a_platform_without_a_source() {
        if root_free_source(std::env::consts::OS).is_some() {
            return;
        }
        let report = probe();
        assert!(report.sensors.is_empty(), "本平台不该有读数：{:?}", report.sensors);
        assert!(report.reason.is_some());
    }

    /// 全局不变量：空报告一定带原因，非空报告一定不带原因。任何一条腿上都成立。
    #[test]
    fn an_empty_report_always_explains_itself() {
        let report = probe();
        if report.sensors.is_empty() {
            let reason = report.reason.expect("空报告必须说清为什么空");
            assert!(reason.chars().count() > 10, "原因太短，等于没说：{reason}");
        } else {
            assert!(report.reason.is_none(), "有读数时不该再挂原因");
        }
    }

    /// 温度与风扇各归各类：风扇的 9000 不能混进温度榜（否则界面会读出"9000 °C"）。
    #[test]
    fn temperature_and_fan_rows_never_swap_kinds() {
        let tree = Tree::new("kinds");
        tree.hwmon(
            "hwmon0",
            &[
                ("name", "mix"),
                ("fan1_input", "9000"),
                ("temp1_input", "40000"),
                ("temp2_input", "78000"),
            ],
        );
        let report = probe_at(&tree.root);
        assert_eq!(report.sensors.len(), 3);
        assert_eq!(report.fans().count(), 1);
        let temps: Vec<f64> = report.temperatures().map(|s| s.value).collect();
        assert_eq!(temps, vec![40.0, 78.0], "温度榜里不该出现转速：{temps:?}");
        assert_eq!(report.sensors.iter().filter(|s| s.kind == SensorKind::Fan).count(), 1);
        assert_eq!(report.sensors.last().unwrap().label, "mix fan1", "同芯片里风扇排在温度之后");
    }

    #[test]
    fn sensor_units_match_the_kind() {
        assert_eq!(SensorKind::Temperature.unit(), "°C");
        assert_eq!(SensorKind::Fan.unit(), "RPM");
    }

    /// 判级只用硬件自己上报的上限：越限 critical、距上限 10 度以内 warning、都没有就 unknown。
    /// `unknown` 必须是一个显式状态 —— 把"这块传感器没说上限"渲染成绿色就是编数据。
    #[test]
    fn severity_uses_only_the_limit_the_hardware_reports() {
        assert_eq!(severity_of(SensorKind::Temperature, 96.0, Some(95.0)), SensorSeverity::Critical);
        assert_eq!(severity_of(SensorKind::Temperature, 95.0, Some(95.0)), SensorSeverity::Critical, "等于上限就是越限");
        assert_eq!(severity_of(SensorKind::Temperature, 85.0, Some(95.0)), SensorSeverity::Warning);
        assert_eq!(severity_of(SensorKind::Temperature, 84.9, Some(95.0)), SensorSeverity::Ok);
        assert_eq!(severity_of(SensorKind::Temperature, 40.0, None), SensorSeverity::Unknown);
        // 风扇没有"上限"，但 0 转是故障
        assert_eq!(severity_of(SensorKind::Fan, 0.0, None), SensorSeverity::Warning);
        assert_eq!(severity_of(SensorKind::Fan, 1_200.0, Some(5_000.0)), SensorSeverity::Ok, "风扇不该按上限判");
    }

    /// 读数与等级必须一起出门（`Sensor::new` 是唯一构造口），且真实采集出来的行也带着等级。
    #[test]
    fn discovered_rows_carry_their_severity_with_them() {
        let tree = Tree::new("severity");
        tree.hwmon(
            "hwmon0",
            &[
                ("name", "chip"),
                ("temp1_label", "过热的那个"),
                ("temp1_input", "96000"),
                ("temp1_max", "95000"),
                ("temp2_label", "没上报上限"),
                ("temp2_input", "40000"),
                ("fan1_label", "停转"),
                ("fan1_input", "0"),
            ],
        );
        let report = probe_at(&tree.root);
        let severity_of_label = |label: &str| {
            report
                .sensors
                .iter()
                .find(|s| s.label == label)
                .unwrap_or_else(|| panic!("没有 {label}：{:?}", report.sensors))
                .severity
        };
        assert_eq!(severity_of_label("过热的那个"), SensorSeverity::Critical);
        assert_eq!(severity_of_label("没上报上限"), SensorSeverity::Unknown);
        assert_eq!(severity_of_label("停转"), SensorSeverity::Warning);
    }

    /// 温度判级只能有一处：后端算好，Agent 与界面都读同一个字段。
    /// 这一条是文本审计 —— 它拦的是"以后有人在别处再写一份阈值"。
    #[test]
    fn the_temperature_verdict_lives_in_exactly_one_place() {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let agent_rs = std::fs::read_to_string(manifest.join("src/agent.rs")).unwrap();
        let agent_production = agent_rs.split("#[cfg(test)]").next().expect("agent.rs 应有测试模块");
        assert!(
            agent_production.contains("sensor.severity"),
            "Agent 该读后端算好的等级，而不是自己比大小"
        );
        assert!(
            !agent_production.contains("HEADROOM"),
            "Agent 里又长出一份温度阈值"
        );

        // 只扫生产段落：这条断言自己就写着那个常量名，连测试一起数等于自己绊自己。
        let thermal_rs = std::fs::read_to_string(manifest.join("src/platform/thermal.rs")).unwrap();
        let thermal_production = thermal_rs
            .split("#[cfg(test)]")
            .next()
            .expect("thermal.rs 应有测试模块");
        assert_eq!(
            thermal_production.matches("WARN_HEADROOM_C: f64").count(),
            1,
            "阈值定义该只有一处"
        );

        let tab = std::fs::read_to_string(manifest.join("../src/components/tabs/SystemInfoTab.tsx")).unwrap();
        assert!(tab.contains("severity"), "系统信息页要按后端给的等级上色");
        assert!(!tab.contains("HEADROOM"), "界面又算了一遍温度阈值");
    }

    /// 状态列的文案只有一个出处，而且分得清"温度偏高"与"风扇停转"。
    /// 浏览器实测里 0 RPM 一度被标成"偏高"（对风扇来说这是反话），所以把措辞的归属钉住。
    #[test]
    fn the_status_wording_has_one_home_and_a_fan_gets_its_own_word() {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let lib_ts = std::fs::read_to_string(manifest.join("../src/lib/thermal.ts")).unwrap();
        let wording = lib_ts
            .split("export function severityText")
            .nth(1)
            .expect("状态文案该集中在 severityText 一个函数里");
        assert!(wording.contains("停转"), "风扇的 warning 要说停转，不能说偏高");
        assert!(wording.contains(r#"kind === "fan""#), "分岔只看传感器种类");

        let tab = std::fs::read_to_string(manifest.join("../src/components/tabs/SystemInfoTab.tsx")).unwrap();
        assert!(tab.contains("severityText(sensor)"), "状态列要走 severityText");
        assert!(
            !tab.contains("SEVERITY_TEXT["),
            "界面不要再自己查表拼文案，否则同一等级会有两套说法"
        );
    }

    /// 后端序列化出来的 key 必须与前端 `interface` 的字段逐一对齐（含两个枚举的取值集合）。
    /// 浏览器里"数字显示对了"不算契约 —— 那次的 JSON 是手工喂进桩的。字段名一旦漂移
    /// （比如 `critical` 改叫 `max`），前端读到 `undefined`：上限列全变 `—`、状态列全空白，
    /// **没有一处会报错**，所以这条只能钉在这里。
    #[test]
    fn the_serialized_thermal_payload_matches_the_frontend_interfaces() {
        use crate::contract_fixtures::{serialized_keys, ts_interface_keys};

        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let contract = std::fs::read_to_string(manifest.join("../src/ipc_contract.ts")).unwrap();

        let sensor = Sensor::new(
            "Package id 0".to_string(),
            SensorKind::Temperature,
            97.0,
            Some(105.0),
            "/sys/class/hwmon/hwmon0/temp1_input".to_string(),
        );
        let rows = serde_json::to_value(&sensor).unwrap();
        assert_eq!(
            serialized_keys(&rows),
            ts_interface_keys(&contract, "ThermalSensor"),
            "Sensor 的字段名与前端 ThermalSensor 不一致"
        );

        let report = ThermalReport {
            sensors: vec![sensor],
            reason: None,
        };
        let envelope = serde_json::to_value(&report).unwrap();
        assert_eq!(
            serialized_keys(&envelope),
            ts_interface_keys(&contract, "ThermalReport"),
            "ThermalReport 的字段名与前端不一致"
        );

        let kinds: Vec<String> = [SensorKind::Temperature, SensorKind::Fan]
            .iter()
            .map(|k| serde_json::to_value(k).unwrap().as_str().unwrap().to_string())
            .collect();
        assert_eq!(kinds, ["temperature", "fan"], "SensorKind 的序列化值与 TS 联合类型漂移了");
        assert!(
            contract.contains(r#"export type SensorKind = "temperature" | "fan";"#),
            "前端 SensorKind 的取值集合变了，上面那条断言就得跟着改"
        );

        let severities: Vec<String> = [
            SensorSeverity::Ok,
            SensorSeverity::Warning,
            SensorSeverity::Critical,
            SensorSeverity::Unknown,
        ]
        .iter()
        .map(|s| serde_json::to_value(s).unwrap().as_str().unwrap().to_string())
        .collect();
        assert_eq!(
            severities,
            ["ok", "warning", "critical", "unknown"],
            "SensorSeverity 的序列化值与 TS 联合类型漂移了"
        );
        assert!(
            contract.contains(r#"export type SensorSeverity = "ok" | "warning" | "critical" | "unknown";"#),
            "前端 SensorSeverity 的取值集合变了"
        );
    }
}


