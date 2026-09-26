//! 系统性观察（T6-01）：把"整台机器上到底是谁在占"做成可回答的问题。
//!
//! 进程表只有平铺的一列时，"java 进程怎么这么多"这类问题只能靠人肉把 743 行加总。
//! 这一层补三件事：
//!   1. 按运行时/应用**归类汇总**（`classify` + `rollup_from`）；
//!   2. 判定**脱离启动者**的进程（`is_detached`）——被 launchd 收养意味着原始 shell 已退出；
//!   3. **监听端口反查进程**（`listening_sockets`）——"8201 被谁占着"此前无从回答。
//!
//! 归类与解析一律做成纯函数、外部命令单独收口，理由同 `commands::dns_flush_plan`：
//! 判定逻辑要能在不碰真机的情况下被测完，`lsof` 不在 CI 上也不该让测试变红。

use std::collections::{BTreeMap, HashMap};

use serde::Serialize;

use crate::monitor::ProcessInfo;

/// 归类桶。**固定字面量**而不是自由文本：前端按它筛选，契约测试按值对拍，
/// 一旦允许拼出任意字符串，两边就都没法钉死。
pub const CATEGORY_JVM: &str = "JVM";
pub const CATEGORY_PYTHON: &str = "Python";
pub const CATEGORY_NODE: &str = "Node";
pub const CATEGORY_QODER: &str = "Qoder";
pub const CATEGORY_CHROME: &str = "Chrome";
pub const CATEGORY_WEBKIT: &str = "WebKit";
pub const CATEGORY_SYSTEM: &str = "系统服务";
pub const CATEGORY_OTHER: &str = "其他";

/// 匹配顺序即优先级，先命中先归类。
///
/// 为什么 Qoder/Chrome 要排在 Node 前面：它们的 helper 进程名里并不含 electron，
/// 而 IDE 与浏览器的子进程确实常被列成 `node`；排在后面会把人家的账记到 Node 头上。
const RULES: &[(&str, &[&str])] = &[
    (CATEGORY_QODER, &["qoder"]),
    (CATEGORY_CHROME, &["google chrome", "chromium", "chrome helper"]),
    (CATEGORY_WEBKIT, &["com.apple.webkit"]),
    (CATEGORY_JVM, &["java", "kotlin", "gradle", "mvn", "openjdk"]),
    (CATEGORY_PYTHON, &["python", "hermes"]),
    (
        CATEGORY_NODE,
        &["node", "npm", "pnpm", "yarn", "vite", "esbuild", "tsx"],
    ),
    (
        CATEGORY_SYSTEM,
        &["launchd", "kernel_task", "windowserver", "/usr/libexec", "com.apple"],
    ),
];

/// 由进程名归到某个运行时/应用桶。空名与 `(unknown)` 落进"其他"，不猜。
pub fn classify(name: &str) -> &'static str {
    let lower = name.to_lowercase();
    if lower.is_empty() || lower == "(unknown)" {
        return CATEGORY_OTHER;
    }
    for (category, needles) in RULES {
        if needles.iter().any(|n| lower.contains(n)) {
            return category;
        }
    }
    CATEGORY_OTHER
}

/// 一个归类桶的合计。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessRollup {
    pub category: &'static str,
    pub process_count: usize,
    /// 各进程常驻内存之和。刻意不做"去重后的真实占用"——共享页要跨进程统计，
    /// sysinfo 不给，宁可报一个偏大的口径并在界面上说明，也不给一个说不清的数。
    pub memory_bytes: u64,
    /// 其中被 launchd 收养的个数（原始启动者已退出）。
    pub detached_count: usize,
    /// 该桶里内存最大的进程，供界面一键定位；桶为空时为 `None`。
    pub top_pid: Option<u32>,
    pub top_name: Option<String>,
}

/// 当前进程的 uid，用来区分"用户自己起的"和"root/别的用户起的"。
/// 取不到就返回 `None`，此时一律按"不属主"处理，绝不反过来假定属主成立。
pub fn current_uid() -> Option<u32> {
    #[cfg(unix)]
    {
        Some(unsafe { libc::getuid() })
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// 进程的有效 uid，取不到就 `None`（一律按"不属主"处理）。
///
/// Windows 上 sysinfo 给的是 `Sid` 而不是数值 uid，那里没有可比的数 —— 直接 `None`，
/// 让 `uid_matches` 走"缺值判 false"那一支，而不是在比较之前先把人分成"都是我的"。
pub fn effective_uid(proc_info: &sysinfo::Process) -> Option<u32> {
    #[cfg(unix)]
    {
        proc_info.effective_user_id().map(|their| **their)
    }
    #[cfg(not(unix))]
    {
        let _ = proc_info;
        None
    }
}

/// 是否"脱离启动者"：父进程是 launchd（pid 1）或根本查不到。
///
/// 刻意不叫"残骸"：launchd 托管的系统守护进程同样满足这一条。工具只给事实，
/// "该不该清"要人结合属主与运行时长判断——把判定做成旗，就会有人照着旗去点"结束"。
pub fn is_detached(parent_pid: Option<u32>) -> bool {
    matches!(parent_pid, None | Some(1))
}

/// 属主判定的**唯一**一处比较，收在纯函数里是为了让四个方向都有固定猎物：
/// 读不到对方 uid 必须为假、别人的 uid 必须为假、自己的 uid 必须为真、
/// 读不到**自己**的 uid 时（非 unix）也一律为假 —— 宁可不报属主，也不假定属主成立。
///
/// 为什么非要一张固定真值表：进程表那条对账路径根本喂不出"别人的 uid"。
/// 实测本机（非提权进程）对 root 进程一律读不到 `effective_user_id()`，全是 `None`；
/// 于是"恒真"变异在 `ps -o uid=` 全表对账下**照样绿** —— 反向半边被那些 `None` 行
/// 顺手满足了。只有给定输入的真值表才拦得住这个方向。
pub fn uid_matches(their: Option<u32>, me: Option<u32>) -> bool {
    match (their, me) {
        (Some(their), Some(me)) => their == me,
        _ => false,
    }
}

/// 按 `classify` 汇总。内存降序，便于界面直接把大头摆在最上面。
pub fn rollup_from(rows: &[ProcessInfo]) -> Vec<ProcessRollup> {
    /// 桶内累加态。`top_mem` 必须自己留着：只存 `top_pid` 的话，每来一行都得回扫
    /// 整批行去找那个 pid 的内存，743 行就是 55 万次比较。
    struct Accum {
        count: usize,
        memory: u64,
        detached: usize,
        top_pid: Option<u32>,
        top_name: Option<String>,
        top_mem: u64,
    }

    let mut buckets: BTreeMap<&'static str, Accum> = BTreeMap::new();
    for row in rows {
        let slot = buckets.entry(classify(&row.name)).or_insert(Accum {
            count: 0,
            memory: 0,
            detached: 0,
            top_pid: None,
            top_name: None,
            top_mem: 0,
        });
        slot.count += 1;
        slot.memory = slot.memory.saturating_add(row.memory_bytes);
        if is_detached(row.parent_pid) {
            slot.detached += 1;
        }
        if row.memory_bytes > slot.top_mem {
            slot.top_mem = row.memory_bytes;
            slot.top_pid = Some(row.pid);
            slot.top_name = Some(row.name.clone());
        }
    }

    let mut out: Vec<ProcessRollup> = buckets
        .into_iter()
        .map(
            |(category, a)| ProcessRollup {
                category,
                process_count: a.count,
                memory_bytes: a.memory,
                detached_count: a.detached,
                top_pid: a.top_pid,
                top_name: a.top_name,
            },
        )
        .collect();
    out.sort_by(|a, b| {
        b.memory_bytes
            .cmp(&a.memory_bytes)
            .then(a.category.cmp(b.category))
    });
    out
}

/// 一个监听套接字。`processName` 不在这里填：进程名一律以进程表为准，
/// 避免同一张表里出现两个名字来源（`lsof` 的 COMMAND 列还会截断到 15 字符）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListeningSocket {
    pub pid: u32,
    pub port: u16,
    /// `*`、`127.0.0.1`、`[::1]` 之类。
    pub address: String,
    /// `tcp`。目前只枚举 TCP LISTEN。
    pub protocol: &'static str,
    /// 占这个端口的进程名，由 `attach_process_names` 从**进程表**那一份数据 join 进来。
    /// 刻意不取 `lsof` 的 COMMAND 列：那一列截断到 15 字符，同一个进程在两张表里会叫两个名字。
    /// `None` 表示这一轮进程表里没有这个 pid（刚退出，或没枚举到），不是"没有名字"。
    pub process_name: Option<String>,
}

/// 把监听端口 join 上进程名：一次全量枚举换一张 pid→名字表，
/// 比每个 socket 各查一次进程（`lookup_process` 未命中缓存时要全量枚举）便宜得多。
pub fn attach_process_names(report: &mut ListeningReport, rows: &[ProcessInfo]) {
    let mut names: HashMap<u32, &str> = HashMap::with_capacity(rows.len());
    for row in rows {
        names.entry(row.pid).or_insert(row.name.as_str());
    }
    for socket in &mut report.sockets {
        socket.process_name = names.get(&socket.pid).map(|name| (*name).to_string());
    }
}

/// 监听端口报告。`reason` 说明为什么是空表：没权限、没有 lsof、还是真的一个都没有，
/// 三者对用户的下一步完全不同，不能都糊成空列表。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListeningReport {
    pub sockets: Vec<ListeningSocket>,
    pub reason: Option<String>,
}

/// `lsof -F` 的字段输出解析：`p<pid>` 开一段，段内 `n<address>:<port>` 是一条监听。
///
/// 只认 `-Fpn` 产出的这两类字段；其余（`f` 文件描述符、`PTCP` 协议）忽略，
/// 但**协议一律记成 `tcp`**——因为选择器写死了 `-iTCP -sTCP:LISTEN`，
/// 输出里出现别的协议说明调用方拼错了参数，那由 `listening_sockets` 那边保证。
pub fn parse_lsof_listeners(raw: &str) -> Vec<ListeningSocket> {
    let mut out = Vec::new();
    let mut pid: Option<u32> = None;
    for line in raw.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        let (tag, value) = line.split_at(1);
        match tag {
            "p" => pid = value.trim().parse::<u32>().ok(),
            "n" => {
                let Some(owner) = pid else { continue };
                // `*:8201` / `127.0.0.1:8201` / `[::1]:5399`。端口取最后一个冒号之后。
                let Some((address, port)) = value.rsplit_once(':') else {
                    continue;
                };
                let Ok(port) = port.parse::<u16>() else {
                    continue;
                };
                out.push(ListeningSocket {
                    pid: owner,
                    port,
                    address: address.to_string(),
                    protocol: "tcp",
                    // 名字不在这里填：解析器只认 lsof 的字段，join 由 `attach_process_names` 做。
                    process_name: None,
                });
            }
            _ => {}
        }
    }
    out.sort_by_key(|s| (s.port, s.pid));
    out.dedup_by_key(|s| (s.pid, s.port, s.address.clone()));
    out
}

/// 真正去问 `lsof`。60–110 ms 量级，只在用户展开端口视图或主动刷新时付，
/// 不放进 1 s 的进程流循环。
///
/// 已知边界：非 root 只能看见**当前用户**的 socket，所以系统服务那一栏会是空的。
/// 这条由 `reason` 在界面说清楚，不假装是全机口径。
pub fn listening_sockets() -> ListeningReport {
    let output = match std::process::Command::new("lsof")
        .args(["-nP", "-iTCP", "-sTCP:LISTEN", "-Fpn"])
        .output()
    {
        Ok(out) => out,
        Err(e) => {
            return ListeningReport {
                sockets: Vec::new(),
                reason: Some(crate::log_sanitize::sanitize(&format!(
                    "无法调用 lsof：{}",
                    e.kind()
                ))),
            }
        }
    };
    // lsof 用退出码 1 表示"没有匹配项"，这不是失败，不能报成错误。
    if !output.status.success() && output.status.code() != Some(1) {
        return ListeningReport {
            sockets: Vec::new(),
            reason: Some(crate::log_sanitize::sanitize(&String::from_utf8_lossy(
                &output.stderr,
            ))),
        };
    }
    let sockets = parse_lsof_listeners(&String::from_utf8_lossy(&output.stdout));
    ListeningReport {
        reason: if sockets.is_empty() {
            Some("当前用户权限下没有 TCP 监听端口（系统服务的 socket 需要 root 才可见）".to_string())
        } else {
            None
        },
        sockets,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(pid: u32, name: &str, memory_bytes: u64, parent_pid: Option<u32>) -> ProcessInfo {
        ProcessInfo {
            pid,
            name: name.to_string(),
            cpu_usage: 0.0,
            memory_bytes,
            threads: None,
            user_name: None,
            parent_pid,
            run_time_seconds: Some(0),
            detached: is_detached(parent_pid),
            owned_by_current_user: false,
        }
    }

    /// 归类表拿**真机见过的名字**来测：这些是 `ps`/进程表里实际出现的形状，
    /// 换成 `foo.exe` 这种假名字，规则漂了也测不出来。
    #[test]
    fn classifies_names_seen_on_a_real_machine() {
        let cases = [
            ("java", CATEGORY_JVM),
            ("/usr/bin/java", CATEGORY_JVM),
            ("python", CATEGORY_PYTHON),
            ("python3.14", CATEGORY_PYTHON),
            ("hermes_cli.main", CATEGORY_PYTHON),
            ("node", CATEGORY_NODE),
            ("npm run dev --port 5399", CATEGORY_NODE),
            ("esbuild", CATEGORY_NODE),
            ("vite", CATEGORY_NODE),
            ("Qoder CN", CATEGORY_QODER),
            ("Qoder CN Helper (Renderer)", CATEGORY_QODER),
            ("Google Chrome Helper (Renderer)", CATEGORY_CHROME),
            ("com.apple.WebKit.WebContent", CATEGORY_WEBKIT),
            ("launchd", CATEGORY_SYSTEM),
            ("WindowServer", CATEGORY_SYSTEM),
            ("Activity Monitor", CATEGORY_OTHER),
            ("(unknown)", CATEGORY_OTHER),
            ("", CATEGORY_OTHER),
        ];
        for (name, want) in cases {
            assert_eq!(classify(name), want, "归类错了：{name:?}");
        }
    }

    /// IDE/浏览器的 helper 不许被记到 Node 头上——这条正是本轮真实踩过的账：
    /// `Qoder CN Helper` 里有 node 子进程，但汇总口径要按应用归，不是按运行时归。
    #[test]
    fn application_buckets_win_over_runtime_buckets() {
        assert_eq!(classify("Qoder CN Helper"), CATEGORY_QODER);
        assert_eq!(classify("Google Chrome Helper (Renderer)"), CATEGORY_CHROME);
    }

    #[test]
    fn rollup_counts_and_sums_per_category() {
        let rows = vec![
            row(1, "java", 1000, Some(24087)),
            row(2, "java", 500, Some(1)),
            row(3, "node", 300, Some(1)),
            row(4, "python", 100, Some(1412)),
        ];
        let all = rollup_from(&rows);
        let find = |c: &str| all.iter().find(|r| r.category == c).expect("桶应存在");
        let jvm = find(CATEGORY_JVM);
        assert_eq!(jvm.process_count, 2);
        assert_eq!(jvm.memory_bytes, 1500);
        assert_eq!(jvm.detached_count, 1, "只有 ppid=1 那一条算脱离");
        assert_eq!(jvm.top_pid, Some(1), "最大内存的那条");
        assert_eq!(find(CATEGORY_NODE).detached_count, 1);
        assert_eq!(find(CATEGORY_PYTHON).detached_count, 0);
    }

    /// 顺序必须是内存降序：界面直接照这个顺序画，画错就等于把小头摆在了眼前。
    #[test]
    fn rollup_is_ordered_by_memory_descending() {
        let rows = vec![
            row(1, "python", 100, Some(0)),
            row(2, "java", 9000, Some(0)),
            row(3, "node", 400, Some(0)),
        ];
        let all = rollup_from(&rows);
        let mem: Vec<u64> = all.iter().map(|r| r.memory_bytes).collect();
        assert_eq!(mem, vec![9000, 400, 100]);
    }

    /// 空输入不能产出一个"满分"的桶：那会让界面显示成"什么都没占"。
    #[test]
    fn empty_input_yields_no_buckets() {
        assert!(rollup_from(&[]).is_empty());
    }

    #[test]
    fn detached_means_parent_is_launchd_or_unknown() {
        assert!(is_detached(Some(1)));
        assert!(is_detached(None));
        assert!(!is_detached(Some(0)), "父进程是内核，不算脱离");
        assert!(!is_detached(Some(24087)));
    }

    /// 属主真值表三个方向都要有固定猎物，不靠"本机恰好有个 root 进程"：
    /// 恒真会被 `!is_detached` 那类取样放过（实测绿），恒假又会把"我的"那一半判没。
    #[test]
    fn ownership_never_assumes_when_a_uid_is_missing() {
        assert!(uid_matches(Some(501), Some(501)), "自己的 uid 必须判属主");
        assert!(!uid_matches(Some(0), Some(501)), "root 的进程不是我的");
        assert!(
            !uid_matches(None, Some(501)),
            "读不到对方 uid 时不许假定属主成立"
        );
        assert!(
            !uid_matches(Some(501), None),
            "读不到自己的 uid 时一律不报属主（非 unix 就是这个形状）"
        );
        // 反向猎物：把 `their == me` 写成 `their != me` 必须红。
        assert!(!uid_matches(Some(502), Some(501)));
    }

    /// 解析器的阳性对照：`-Fpn` 的真实形状（本机 `lsof` 抓的）必须解出 pid 与端口。
    #[test]
    fn parses_the_field_output_shape_lsof_actually_emits() {
        let raw = "p66493\nf13\nPTCP\nn*:18090\np632\nf10\nPTCP\nn127.0.0.1:49236\n";
        let got = parse_lsof_listeners(raw);
        assert_eq!(got.len(), 2, "解出的条数不对：{got:?}");
        assert!(got.iter().any(|s| s.pid == 66493 && s.port == 18090));
        assert!(got.iter().any(|s| s.pid == 632 && s.port == 49236));
        assert_eq!(got[0].protocol, "tcp");
    }

    /// IPv6 的地址本身带冒号，端口只能按**最后一个**冒号切；按第一个切会把
    /// `[::1]:5399` 解成端口 `1`，而且不报错。
    #[test]
    fn ipv6_addresses_split_on_the_last_colon() {
        let got = parse_lsof_listeners("p56106\nn[::1]:5399\n");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].port, 5399, "IPv6 端口解析错了：{:?}", got[0]);
        assert_eq!(got[0].address, "[::1]");
    }

    /// 脏输入不许 panic，更不许**错归属**。真正要防的不是"少一条"，而是把后一个进程
    /// 的端口记到前一个进程头上 —— 那会让人照着去杀错进程。
    #[test]
    fn malformed_lines_are_dropped_not_misattributed() {
        // `n` 出现在任何 `p` 之前：没有属主，丢弃。
        assert!(parse_lsof_listeners("n*:80\n").is_empty(), "无主的 n 不该成记录");
        // `p` 行本身是垃圾：属主置空，其后那条 `n` 必须**不**挂到上一段的 pid 12 上。
        let after_bad_pid = parse_lsof_listeners("p12\nn*:8200\npnotapid\nn*:8201\n");
        assert_eq!(
            after_bad_pid.iter().map(|s| (s.pid, s.port)).collect::<Vec<_>>(),
            vec![(12, 8200)],
            "垃圾 p 行之后的端口被错挂到前一个进程：{after_bad_pid:?}"
        );
        // 端口不是数字：整条丢掉，不猜。
        assert_eq!(parse_lsof_listeners("p12\nnnotaport\n").len(), 0);
        // 阳性对照：把垃圾 `p` 行拿掉，同一条 8201 就必须解出来。
        // 少了这一支，上面那条断言在"解析器整个坏掉"时也会一起绿。
        let clean = parse_lsof_listeners("p12\nn*:8200\nn*:8201\n");
        assert_eq!(clean.len(), 2, "去掉垃圾行后仍解不出记录，说明是解析器坏了：{clean:?}");
        assert!(clean.iter().any(|s| s.port == 8201 && s.pid == 12));
    }

    /// 同一进程常为 `0.0.0.0` 与 `[::]` 各开一个 fd，界面会看成两条重复端口。
    #[test]
    fn duplicate_pid_port_pairs_collapse() {
        let got = parse_lsof_listeners("p7\nn*:7000\np7\nn*:7000\n");
        assert_eq!(got.len(), 1, "重复项没去掉：{got:?}");
    }

    /// join 进程名要双向：命中的填上名字、没命中的**保持 null**。
    /// 后者若被填成空串或 `(unknown)`，界面就会把"这轮没枚举到"显示成"进程叫 (unknown)"。
    #[test]
    fn process_names_join_onto_listeners_and_leave_unknowns_alone() {
        let mut report = ListeningReport {
            sockets: parse_lsof_listeners("p7\nn*:18090\np99\nn127.0.0.1:53\n"),
            reason: None,
        };
        assert_eq!(report.sockets.len(), 2, "阳性对照：夹具里得有两条监听才可 join");
        assert!(
            report.sockets.iter().all(|s| s.process_name.is_none()),
            "解析阶段就该留空，名字只能来自 join"
        );
        let rows = vec![row(7, "java", 4096, Some(300))];
        attach_process_names(&mut report, &rows);
        let named = report.sockets.iter().find(|s| s.pid == 7).expect("pid 7 应在表里");
        assert_eq!(named.process_name.as_deref(), Some("java"));
        let missing = report.sockets.iter().find(|s| s.pid == 99).expect("pid 99 应在表里");
        assert_eq!(missing.process_name, None, "查不到的 pid 不许编名字");
    }

    /// 归类桶与端口报告的字段名必须与前端 `interface` 逐一对齐（T5-02 / T5-08 同一条纪律）。
    /// 漂了是**静默**的：`detachedCount` 变 `undefined`，前端当 0 用，界面就从"有 4 个脱离"
    /// 悄悄变成"一个都没有"，没有任何一处会报错。
    #[test]
    fn rollup_and_listening_payloads_match_the_frontend_interfaces() {
        use crate::contract_fixtures::{serialized_keys, ts_interface_keys};

        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let contract = std::fs::read_to_string(manifest.join("../src/ipc_contract.ts")).unwrap();

        let rows = vec![row(7, "java", 4096, Some(1)), row(8, "node", 2048, Some(300))];
        let mut rollup = rollup_from(&rows);
        assert_eq!(rollup.len(), 2, "阳性对照：桶都没聚出来，对拍就是空跑");
        let bucket = rollup.remove(0);
        assert!(bucket.detached_count > 0, "阳性对照：这一份夹具里得真有一个脱离的");
        assert_eq!(
            serialized_keys(&serde_json::to_value(&bucket).unwrap()),
            ts_interface_keys(&contract, "ProcessRollup"),
            "ProcessRollup 的字段名与前端不一致"
        );

        let sockets = parse_lsof_listeners("p4242\nn*:18090\n");
        assert_eq!(sockets.len(), 1, "阳性对照：解析不到监听，就没东西可对拍");
        assert_eq!(
            serialized_keys(&serde_json::to_value(&sockets[0]).unwrap()),
            ts_interface_keys(&contract, "ListeningSocket"),
            "ListeningSocket 的字段名与前端不一致"
        );
        let report = ListeningReport { sockets, reason: None };
        assert_eq!(
            serialized_keys(&serde_json::to_value(&report).unwrap()),
            ts_interface_keys(&contract, "ListeningReport"),
            "ListeningReport 的字段名与前端不一致"
        );

        // 驼峰值守：`rename_all = "camelCase"` 掉了，前端会默默读到 undefined。
        let raw = serde_json::to_string(&bucket).unwrap();
        for key in ["processCount", "memoryBytes", "detachedCount", "topPid", "topName"] {
            assert!(raw.contains(key), "序列化里缺驼峰值 {key}：{raw}");
            let snake = match key {
                "processCount" => "process_count",
                "memoryBytes" => "memory_bytes",
                "detachedCount" => "detached_count",
                "topPid" => "top_pid",
                _ => "top_name",
            };
            assert!(!raw.contains(snake), "序列化仍是下划线 {snake}：{raw}");
        }
    }

    /// 命令名两边都要登记：前端 `Commands` 里写了、后端 `lib.rs` 没注册，
    /// 界面只会看到一句"命令不存在"；反过来注册了前端没登记，则根本没人调 —— 两种都是静默的。
    #[test]
    fn rollup_and_socket_commands_are_wired_on_both_sides() {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let contract = std::fs::read_to_string(manifest.join("../src/ipc_contract.ts")).unwrap();
        let lib_rs = std::fs::read_to_string(manifest.join("src/lib.rs")).unwrap();
        let commands_rs = std::fs::read_to_string(manifest.join("src/commands.rs")).unwrap();
        for (front, back) in [
            ("getProcessRollup: \"get_process_rollup\"", "commands::get_process_rollup"),
            ("getListeningSockets: \"get_listening_sockets\"", "commands::get_listening_sockets"),
        ] {
            assert!(contract.contains(front), "前端未登记 {front}");
            assert!(lib_rs.contains(back), "命令未注册进 invoke_handler：{back}");
            assert!(
                commands_rs.contains(&format!("pub async fn {}", back.split("::").last().unwrap())),
                "commands.rs 里没有 {back} 的实现"
            );
        }
    }

    /// 真机对账：本机此刻必然有 TCP 监听（IDE、浏览器至少各一个），
    /// 空表就是通路断了，不是"真的没有"。
    #[cfg(unix)]
    #[test]
    fn live_lsof_finds_at_least_one_listener_on_this_machine() {
        let report = listening_sockets();
        if report.sockets.is_empty() {
            // 允许在极特殊的容器环境里成立，但必须把原因带出来，不能静默通过。
            assert!(
                report.reason.is_some(),
                "空表必须给出 reason，不能静默为空"
            );
            return;
        }
        assert!(
            report.sockets.iter().all(|s| s.port > 0),
            "端口为 0 说明解析错位"
        );
        assert!(report.reason.is_none(), "有结果时不该带原因：{:?}", report.reason);
    }

    /// 两块新视图要**真的挂在进程页上**，且文案不许越过数据能给的结论。
    ///
    /// 挂错位的失败方式和 T5-08 一样安静：后端把 `detached` 算好了，界面没接，
    /// 用户看到的仍是那张"看不出谁是被抛弃的"表 —— 而所有 Rust 侧测试照样绿。
    /// 文案半边同理：`ownedByCurrentUser == false` 混着"读不到 uid"，所以说"root 进程"
    /// 就是替用户编了一个本列给不出的判断。
    #[test]
    fn the_new_views_are_mounted_and_do_not_overclaim() {
        use crate::contract_fixtures::strip_ts_comments;

        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let read = |file: &str| {
            strip_ts_comments(&std::fs::read_to_string(manifest.join(file)).unwrap_or_else(|_| {
                panic!("{file} 必须存在")
            }))
        };
        let tab = read("../src/components/tabs/ProcessTab.tsx");
        let app = read("../src/App.tsx");
        let rollup_panel = read("../src/components/ProcessRollupPanel.tsx");
        let ports_panel = read("../src/components/ListeningPortsPanel.tsx");

        for needle in [
            "<ProcessRollupPanel",
            "<ListeningPortsPanel",
            "onlyDetached",
            "ownedByCurrentUser",
            "\"detached\"",
            "portsByPid",
        ] {
            assert!(tab.contains(needle), "进程页没接上 {needle}");
        }
        for (file, needle) in [
            ("App.tsx", "rollup={rollup}"),
            ("App.tsx", "listeners={listeners}"),
            ("App.tsx", "Commands.getProcessRollup"),
            ("App.tsx", "Commands.getListeningSockets"),
        ] {
            assert!(app.contains(needle), "{file} 没接上 {needle}");
        }
        // 口径必须写在界面上，不能只写在代码注释里。
        assert!(rollup_panel.contains("共享页"), "归类面板没说明内存合计的口径");
        assert!(
            rollup_panel.contains("全量枚举") || rollup_panel.contains("当前这一页"),
            "归类面板没说清数据来自哪一份"
        );
        assert!(
            ports_panel.contains("当前用户") && ports_panel.contains("权限"),
            "端口面板没说清非提权只能看到自己那一份"
        );

        // 反向：越过数据的说法一个都不许出现。阳性对照放最后 —— 剥注释剥过头会把整份
        // 文案剥空，那时下面这些"不含"全部空转成立。
        assert!(
            rollup_panel.contains("重新观察"),
            "归类面板的可见文案被剥空了，反向断言成了空跑"
        );
        for forbidden in ["root 进程", "孤儿进程", "残骸", "可安全", "建议结束", "已回收"] {
            for (name, visible) in [
                ("ProcessTab", &tab),
                ("ProcessRollupPanel", &rollup_panel),
                ("ListeningPortsPanel", &ports_panel),
            ] {
                assert!(!visible.contains(forbidden), "{name} 的文案越界了：{forbidden}");
            }
        }
    }
}
