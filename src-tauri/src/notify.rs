//! 系统通知（T5-02）：告警除了界面 Toast，再往操作系统的通知中心投一条。
//!
//! 这里**没有第二条"要不要告警"的判断** —— 判定只在 [`crate::alert`] 的引擎里，
//! 引擎已经决定"这是一条告警"之后才会调进来。所以本模块只做两件事：把那条告警写成
//! 一句人话、投出去、把能拿到的结果记下来。
//!
//! 三条从源码里量出来的事实，决定了这里的口径：
//!
//! 1. **走 Rust API，不走前端 JS。** `@tauri-apps/plugin-notification` 的
//!    `sendNotification()` 返回 `void`（内部那条 `invoke` 没被 await），前端拿不到任何结果；
//!    Rust 侧 [`NotificationExt`] 的 `builder().show()` 返回 `Result`，至少"这条有没有被
//!    通知后端接走"是量得到的。也因此 **capabilities 一条权限都不用加**（JS 通道根本没接），
//!    见 `capabilities_stay_the_explicit_eight_without_any_notification_entry`。
//! 2. **插件在桌面端读不到真实授权状态。** `tauri-plugin-notification` 2.4.0 的
//!    `request_permission()` / `permission_state()` 是写死 `Ok(Granted)` 的桩
//!    （`src/desktop.rs:61-67`）。所以界面不许出现"已授权/未授权"这种说法。
//! 3. **macOS 上连投递错误都拿不到。** 默认后端（notify-rust 4.18 的
//!    `nsusernotifications`）是在 `NotificationHandle::drop()` 里才真正发送，并且
//!    `.ok()` 把错误丢掉（`src/macos/nsusernotifications.rs:142-161`）。
//!    ⇒ `show()` 返回 `Ok` 只代表"交给了通知接口"，**不代表用户看到了**。
//!    界面文案因此只说"已提交 N 条"，并把"能不能真弹出来"交回用户自己确认
//!    （抽屉里那条"发一条测试通知"就是干这个的）。

use crate::alert::{AlertEvent, AlertLevel, AlertMetric};
use crate::log_sanitize::sanitize;
use serde::Serialize;
use std::sync::{Arc, Mutex};
use tauri::AppHandle;
use tauri_plugin_notification::NotificationExt;

/// 通知标题的前缀。Toast 是单行的（`lib/alert.ts` 的 `alertText`），系统通知需要一个标题，
/// 故只补这一层；正文与 Toast 逐字同形，由 `the_notification_body_mirrors_the_frontend_toast` 钉住。
pub const TITLE_PREFIX: &str = "资源告警";

fn metric_label(metric: AlertMetric) -> &'static str {
    match metric {
        AlertMetric::Cpu => "CPU 使用率",
        AlertMetric::Memory => "内存使用率",
        AlertMetric::Disk => "磁盘使用率",
    }
}

fn level_label(level: AlertLevel) -> &'static str {
    match level {
        AlertLevel::Warning => "警告",
        AlertLevel::Critical => "严重",
    }
}

/// 一条告警的通知文案 `(标题, 正文)`。
///
/// 磁盘告警的挂载点与落盘副本同一口径：过 `sanitize`（家目录折成 `<user>`）。
/// CPU / 内存没有目标，正文里就不会多出一对空括号。
pub fn alert_texts(event: &AlertEvent) -> (String, String) {
    let level = level_label(event.level);
    let title = format!("{TITLE_PREFIX} · {level}");
    let target = match &event.target {
        Some(path) => format!("（{}）", sanitize(path)),
        None => String::new(),
    };
    let body = format!(
        "{}{target} 连续 {} 帧越过{level}阈值：{:.1} % ≥ {:.0} %",
        metric_label(event.metric),
        event.consecutive,
        event.value,
        event.threshold
    );
    (title, body)
}

/// 测试通知的文案。不带任何采集值 —— 它只回答"这台机器的通知通道能不能把一条消息送到你眼前"。
pub fn test_texts() -> (String, String) {
    (
        format!("{TITLE_PREFIX} · 测试"),
        "这是一条测试通知，不含任何采集数据。看到它说明系统通知可用；没看到请查系统设置里的通知权限。".to_string(),
    )
}

/// 投递结果的记账。字段名即界面看到的：只有"提交"与"失败"，没有"已送达"。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotifyStatus {
    /// 交给通知接口的条数。**不等于"用户看到了 N 条"**，理由见模块头的第 3 条事实。
    pub submitted: u32,
    /// 通知接口当场返回 `Err` 的条数（macOS 那条路几乎总是拿不到错误，见第 3 条）。
    pub failed: u32,
    /// 最后一次失败的原文（过 `sanitize`）。从没失败过时是 `None` —— 界面要显示"没有失败记录"，
    /// 不能拿 `null` 当成"投递成功"。
    pub last_error: Option<String>,
    /// 本平台的通知后端能不能把"投递失败"告诉我们。macOS 的默认后端是否定的（`.ok()` 丢了错误）。
    /// 界面据此决定要不要挂那句"提交不等于弹出"的说明。
    pub delivery_is_reported: bool,
}

/// 默认值里有一项是按目标平台算的（macOS 拿不到投递结果）。这里不用 derive：
/// 派生出来的 `Default` 会把 `deliveryIsReported` 塌成 `false`，而这个字段为 `false` 时
/// 界面要多挂那句说明 —— 在 macOS 上恰好与派生值相同，clippy 因此会"看不出差别"地建议改成 derive，
/// 换到 Linux/Windows 腿又不会。把它写成显式 impl 并要求读者看一眼平台判断，比让门禁在三条腿上各说各话诚实。
#[allow(clippy::derivable_impls)]
impl Default for NotifyStatus {
    fn default() -> Self {
        Self {
            submitted: 0,
            failed: 0,
            last_error: None,
            delivery_is_reported: cfg!(not(target_os = "macos")),
        }
    }
}

/// managed state：采集线程写、`notify_status` 读。
#[derive(Debug, Clone, Default)]
pub struct NotifyState(Arc<Mutex<NotifyStatus>>);

impl NotifyState {
    /// 记一次投递结果。锁中毒（有人 panic 在临界区里）时**只丢这一笔记账**，
    /// 绝不能让"通知没记上"把采集循环带崩 —— 告警本身已经推出去、也已经落盘。
    pub fn record(&self, outcome: &Result<(), String>) {
        let Ok(mut guard) = self.0.lock() else {
            return;
        };
        guard.submitted += 1;
        if let Err(reason) = outcome {
            guard.failed += 1;
            guard.last_error = Some(reason.clone());
        }
    }

    pub fn status(&self) -> NotifyStatus {
        self.0.lock().map(|g| g.clone()).unwrap_or_default()
    }
}

/// 投一条系统通知，并把结果记进 `state`。返回的 `Result` 只用于日志；
/// **调用方（采集循环）不许因为它而中断告警链路。**
///
/// 文案在投递前就拼好了（`texts`），所以调用方能拿到"到底发了什么"，测试也能直接断言文本。
pub fn post<R: tauri::Runtime>(
    app: &AppHandle<R>,
    state: &NotifyState,
    texts: (String, String),
) -> Result<(), String> {
    let (title, body) = texts;
    let outcome = app
        .notification()
        .builder()
        .title(title)
        .body(body)
        .show()
        .map_err(|e| sanitize(&e.to_string()));
    // 与 `monitor.rs` 里 `[alert]` 那行同口径：只在 debug 构建里打，且打的是已经脱敏后的结果。
    // 这条日志是本任务在 macOS 上唯一能拿到"通知后端到底怎么答复"的通道（界面上不显示）。
    #[cfg(debug_assertions)]
    eprintln!("[notify] {outcome:?}");
    state.record(&outcome);
    outcome
}

/// 告警事件专用的投递入口：文案由 [`alert_texts`] 生成，判定不在这里发生。
pub fn post_alert<R: tauri::Runtime>(app: &AppHandle<R>, state: &NotifyState, event: &AlertEvent) {
    let _ = post(app, state, alert_texts(event));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(metric: AlertMetric, level: AlertLevel, target: Option<&str>) -> AlertEvent {
        AlertEvent {
            metric,
            level,
            value: 98.65,
            threshold: 95.0,
            target: target.map(|t| t.to_string()),
            consecutive: 3,
            timestamp_ms: 1,
        }
    }

    /// 正文与前端 Toast 用的 `alertText()` 逐字同形（同一个句子形状、同一批标签词）。
    /// 两条通道说的是同一件事，措辞漂移就会变成"界面写严重、通知写警告"。
    #[test]
    fn the_notification_body_mirrors_the_frontend_toast() {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let alert_ts = std::fs::read_to_string(manifest.join("../src/lib/alert.ts")).unwrap();
        // 标签词只能有一处：后端这几个字面量必须逐个出现在前端那份标签表里。
        for label in ["CPU 使用率", "内存使用率", "磁盘使用率", "警告", "严重"] {
            assert!(
                alert_ts.contains(&format!("\"{label}\"")) || alert_ts.contains(label),
                "前端标签表里没有 {label}，两边措辞已经分岔"
            );
        }
        // 句子形状里的每个连接词也是共享的：模板必须在前端逐字存在。
        let template = alert_ts
            .split("export function alertText")
            .nth(1)
            .expect("前端应有 alertText")
            .lines()
            .find(|line| line.contains("return `"))
            .expect("alertText 应有一行模板")
            .to_string();
        for piece in ["连续 ", " 帧越过", "阈值：", " % ≥ ", " %"] {
            assert!(
                template.contains(piece),
                "前端 Toast 模板里找不到 {piece:?}，两条通道的句子已经分岔"
            );
        }
        let (title, body) = alert_texts(&event(AlertMetric::Disk, AlertLevel::Critical, Some("/Volumes/Data")));
        assert_eq!(title, "资源告警 · 严重");
        assert_eq!(body, "磁盘使用率（/Volumes/Data） 连续 3 帧越过严重阈值：98.7 % ≥ 95 %");
    }

    /// 三个指标、两个档位都得有自己的词，不许出现"两个档位都叫警告"这种含糊。
    #[test]
    fn every_metric_and_level_has_its_own_word() {
        let bodies: Vec<String> = [
            (AlertMetric::Cpu, "CPU 使用率"),
            (AlertMetric::Memory, "内存使用率"),
            (AlertMetric::Disk, "磁盘使用率"),
        ]
        .into_iter()
        .map(|(metric, label)| {
            let (_, body) = alert_texts(&event(metric, AlertLevel::Warning, None));
            assert!(body.starts_with(label), "{label} 没出现在正文开头：{body}");
            assert!(body.contains("警告阈值"), "{label} 的档位词不对：{body}");
            body
        })
        .collect();
        assert_eq!(bodies.len(), 3);
        let critical = alert_texts(&event(AlertMetric::Cpu, AlertLevel::Critical, None)).1;
        assert!(critical.contains("严重阈值") && !critical.contains("警告阈值"), "{critical}");
    }

    /// 没有目标的告警不能凭空多出一对括号（那会被读成"目标是空字符串"）。
    #[test]
    fn a_missing_target_adds_no_empty_parentheses() {
        let (_, body) = alert_texts(&event(AlertMetric::Cpu, AlertLevel::Warning, None));
        assert!(!body.contains('（') && !body.contains('）'), "{body}");
    }

    /// 挂载点与落盘副本走同一个 sanitize：家目录不许整条出现在通知正文里。
    #[test]
    fn the_disk_target_is_sanitized_the_same_way_as_the_persisted_copy() {
        let home = dirs::home_dir().and_then(|p| p.to_str().map(|s| s.to_string()));
        let Some(home) = home else {
            return;
        };
        let target = format!("{home}/Volumes/外置盘");
        let (_, body) = alert_texts(&event(AlertMetric::Disk, AlertLevel::Critical, Some(&target)));
        assert!(!body.contains(&home), "家目录整条漏进通知正文：{body}");
        assert!(body.contains("<user>"), "脱敏后的挂载点应带 <user> 标记：{body}");
    }

    /// 数值精度也是口径的一部分：正文保留一位小数、阈值取整数档，与前端 `toFixed(1)/toFixed(0)` 对齐。
    /// 取整方向是量过的，不是猜的：`98.65` 两侧都得 `98.7`（Rust `{:.1}` 与 JS
    /// `(98.65).toFixed(1)` 实测同为 98.7；`0.05 / 1.25 / 2.35` 四个点位也逐个比过）。
    #[test]
    fn the_numbers_keep_the_same_precision_as_the_toast() {
        let (_, body) = alert_texts(&event(AlertMetric::Memory, AlertLevel::Warning, None));
        assert!(body.contains("98.7 % ≥ 95 %"), "{body}");
        assert!(!body.contains("98.65"), "正文里出现了未取整的原始值：{body}");
    }

    /// 记账只有"提交/失败"，而且失败必须留着原文，否则用户无从分辨"没告警"与"通知没通路"。
    #[test]
    fn a_failed_delivery_is_counted_and_keeps_its_reason() {
        let state = NotifyState::default();
        state.record(&Ok(()));
        state.record(&Err("通知后端拒绝：not authorized".to_string()));
        let status = state.status();
        assert_eq!(status.submitted, 2, "两次调用都该记成已提交");
        assert_eq!(status.failed, 1);
        assert_eq!(status.last_error.as_deref(), Some("通知后端拒绝：not authorized"));
    }

    /// 从没失败过时 `lastError` 是 `None`：界面要显示"没有失败记录"，不许当成"全都送达"。
    #[test]
    fn a_clean_run_reports_no_error_rather_than_a_claim_of_delivery() {
        let state = NotifyState::default();
        state.record(&Ok(()));
        let status = state.status();
        assert_eq!(status.failed, 0);
        assert!(status.last_error.is_none());
        assert_eq!(status.submitted, 1);
        assert!(
            !status.delivery_is_reported || status.last_error.is_some() || status.submitted > 0,
            "只有平台真能报投递结果时才可以省掉这句说明"
        );
    }

    /// macOS 这一路**不能**声称拿得到投递结果（notify-rust 在 Drop 里发送且 `.ok()` 丢错误）。
    #[test]
    fn macos_may_not_claim_that_it_knows_the_delivery_result() {
        let status = NotifyStatus::default();
        if cfg!(target_os = "macos") {
            assert!(!status.delivery_is_reported, "macOS 上这条是谎话：投递结果拿不到");
        }
    }

    /// 计数器不是无限的：它是 `u32`，一次运行里靠 60 s 冷却最多每分钟一条，但记账逻辑不该假设"永远不为 0"。
    /// 这条盯着"提交数只增不减"，防止哪天有人加一个"清零"按钮把历史抹了却不说明。
    #[test]
    fn the_counters_only_ever_move_forward() {
        let state = NotifyState::default();
        let mut previous = state.status().submitted;
        for _ in 0..5 {
            state.record(&Ok(()));
            let now = state.status().submitted;
            assert_eq!(now, previous + 1);
            previous = now;
        }
        assert_eq!(state.status().failed, 0);
    }

    /// 接线：插件注册了、命令在前端登记了、采集循环里只有那一处投递点。
    /// 少任何一处都会让"开了告警却没有通知"变成一条查不出来的哑故障。
    #[test]
    fn the_delivery_path_is_wired_on_both_sides() {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let lib_rs = std::fs::read_to_string(manifest.join("src/lib.rs")).unwrap();
        assert!(lib_rs.contains("mod notify;"), "notify 模块没接进 lib.rs");
        assert!(
            lib_rs.contains("tauri_plugin_notification::init()"),
            "插件没注册，app.notification() 会 panic"
        );
        assert!(lib_rs.contains("commands::notify_status"), "notify_status 没注册");
        let monitor_rs = std::fs::read_to_string(manifest.join("src/monitor.rs")).unwrap();
        assert_eq!(
            monitor_rs.matches("notify::post_alert(").count(),
            1,
            "采集循环里的投递点必须恰好一处：多了就是第二条告警通路"
        );
        let contract = std::fs::read_to_string(manifest.join("../src/ipc_contract.ts")).unwrap();
        assert!(
            contract.contains("notifyStatus: \"notify_status\""),
            "前端未登记 notify_status"
        );
        assert!(
            contract.contains("sendTestNotification: \"send_test_notification\""),
            "前端未登记 send_test_notification"
        );
    }

    /// 这条任务的授权门槛本来是"扩 capabilities"。实测走 Rust API 之后一条权限都不需要，
    /// 于是把它钉成回归：谁哪天改走 JS 通道，必须显式改动这条测试并说清理由。
    #[test]
    fn capabilities_stay_the_explicit_eight_without_any_notification_entry() {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let raw = std::fs::read_to_string(manifest.join("capabilities/default.json")).unwrap();
        let doc: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let perms: Vec<&str> = doc["permissions"]
            .as_array()
            .expect("permissions 应是数组")
            .iter()
            .map(|p| p.as_str().expect("权限项应是字符串"))
            .collect();
        assert_eq!(
            perms,
            vec![
                "core:default",
                "core:event:default",
                "core:event:allow-listen",
                "core:event:allow-emit",
                "dialog:default",
                "dialog:allow-message",
                "dialog:allow-ask",
                "dialog:allow-confirm",
            ],
            "capabilities 变了：T5-02 走的是 Rust API，不需要任何新权限"
        );
        assert!(
            !perms.iter().any(|p| p.contains("notification")),
            "系统通知走的是后端 Rust API；出现 notification 权限说明有人改走了 JS 通道"
        );
        assert!(
            !raw.contains('*'),
            "权限清单里不许出现通配项"
        );
    }

    /// 测试通知不许接任何参数：否则它就成了一条"前端可指定任意文本进系统通知"的通道。
    #[test]
    fn the_test_command_takes_no_arguments() {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let commands_rs = std::fs::read_to_string(manifest.join("src/commands.rs")).unwrap();
        let signature = commands_rs
            .split("pub fn send_test_notification")
            .nth(1)
            .expect("应有 send_test_notification")
            // 只看参数表：到第一个 `{` 为止，别把函数体里的 `test_texts()` 也算进来
            .split('{')
            .next()
            .expect("签名应有参数表")
            .to_string();
        assert!(
            signature.contains("app: tauri::AppHandle"),
            "命令要拿 AppHandle 才投得出通知：{signature}"
        );
        for forbidden in ["text", "title", "body", "message", "config"] {
            assert!(
                !signature.contains(forbidden),
                "测试通知不许接收 {forbidden} 参数（会变成任意文本注入通道）"
            );
        }
        let (title, body) = test_texts();
        assert!(title.contains("测试") && body.contains("不含任何采集数据"), "{title} / {body}");
    }

    /// 记账载荷的字段名必须与前端 `interface NotifyStatus` 逐一对齐。
    /// 漂了的后果和 T5-08 那条一样安静：`deliveryIsReported` 变成 `undefined` ⇒ 前端把它当
    /// `false`，那句"提交不等于弹出"的说明就消失了，界面开始把"已提交"说成"已送达"。
    #[test]
    fn the_notify_status_payload_matches_the_frontend_interface() {
        use crate::contract_fixtures::{serialized_keys, ts_interface_keys};

        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let contract = std::fs::read_to_string(manifest.join("../src/ipc_contract.ts")).unwrap();
        let status = NotifyState::default().status();
        assert_eq!(
            serialized_keys(&serde_json::to_value(&status).unwrap()),
            ts_interface_keys(&contract, "NotifyStatus"),
            "NotifyStatus 的字段名与前端不一致"
        );
        // 驼峰值守：`camelCase` 没生效时这里会先炸，而不是让前端默默读到 undefined。
        let raw = serde_json::to_string(&status).unwrap();
        assert!(raw.contains("deliveryIsReported"), "{raw}");
        assert!(!raw.contains("delivery_is_reported"), "{raw}");
    }

    /// 界面上不许出现"已送达"或"已授权"这类我们其实拿不到的说法（模块头第 2、3 条事实）。
    /// 只扫用户看得见的文案：解释"为什么不许说"的那些注释本身带着这些词，扫全文等于自己绊自己。
    #[test]
    fn the_drawer_wording_never_promises_delivery_or_authorisation() {
        use crate::contract_fixtures::strip_ts_comments;

        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let raw = std::fs::read_to_string(manifest.join("../src/components/AlertSettingsDrawer.tsx")).unwrap();
        let visible = strip_ts_comments(&raw);
        for forbidden in ["已送达", "已授权", "通知成功"] {
            assert!(
                !visible.contains(forbidden),
                "界面文案里出现了 {forbidden}：这条结果本应用拿不到"
            );
        }
        assert!(visible.contains("已提交"), "界面要说的是提交条数，不是别的");
        assert!(
            visible.contains("deliveryIsReported"),
            "拿不到投递结果时那句说明必须由这个开关驱动，否则它会在某个平台上变成谎话"
        );
        // 两句失败语境必须分开：合成一条会写出"系统通知状态读取失败：系统通知投递失败：…"
        // 这种重复的话（浏览器实测抓到过）。
        assert!(
            visible.contains("测试通知没发出去") && visible.contains("读不到系统通知的投递记账"),
            "界面要分得清是「投递失败」还是「读记账失败」"
        );

        // 后端只给原文，不负责造句：否则语境会在两侧各加一半。只看函数体，
        // 那条解释性的文档注释本身就写着这几个字。
        let commands_rs = std::fs::read_to_string(manifest.join("src/commands.rs")).unwrap();
        let body = commands_rs
            .split("pub fn send_test_notification")
            .nth(1)
            .expect("应有 send_test_notification")
            .split("\n}")
            .next()
            .expect("函数体应有结尾")
            .to_string();
        assert!(
            !body.contains("系统通知") && !body.contains("失败"),
            "命令体里不该再拼一句中文说明，交给界面说：{body}"
        );
    }

    /// 系统通知不许成为第二条"判定"通路：除了采集循环里那一处，前端不许任何地方自己拼告警文案。
    #[test]
    fn no_frontend_path_builds_its_own_alert_notice() {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let drawer =
            std::fs::read_to_string(manifest.join("../src/components/AlertSettingsDrawer.tsx")).unwrap();
        assert!(
            !drawer.contains("new Notification") && !drawer.contains("plugin-notification"),
            "前端不许再开一条通知通路：那样会有第二个「要不要通知」的判断"
        );
        let hook = std::fs::read_to_string(manifest.join("../src/hooks/useAlerts.ts")).unwrap();
        assert!(
            !hook.contains("new Notification") && !hook.contains("plugin-notification"),
            "同上：告警链路的前端半边只读记账、不自己发通知"
        );
    }

    /// 计数器跨线程不许丢数：写端在采集线程、读端在 IPC 线程，这是本任务唯一的真并发面。
    /// 少了这条，把 `Mutex` 换成 `Cell`/无锁 `usize` 在单线程测试里照样全绿。
    #[test]
    fn concurrent_records_do_not_lose_counts() {
        let state = NotifyState::default();
        let threads: Vec<_> = (0..8)
            .map(|i| {
                let state = state.clone();
                std::thread::spawn(move || {
                    for _ in 0..50 {
                        // 一半成功一半失败，两种记账路径都要被并发打过
                        let outcome = if i % 2 == 0 {
                            Ok(())
                        } else {
                            Err(format!("线程 {i} 的失败"))
                        };
                        state.record(&outcome);
                    }
                })
            })
            .collect();
        for handle in threads {
            let _ = handle.join();
        }
        let status = state.status();
        assert_eq!(status.submitted, 8 * 50, "并发写丢了计数");
        assert_eq!(status.failed, 4 * 50, "失败计数与提交数不同源");
        assert!(status.last_error.is_some(), "有失败却没留下原文");
    }
}
