# z-biz-tool-sys

> 一款轻量、实时的跨平台系统监控仪表盘，参考 [iStat Menus](https://bjango.com/istatmenus/) 和 [Stats](https://github.com/exelban/stats) 的产品形态，基于 Tauri 打造。

![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Windows%20%7C%20Linux-blue)
![Tech](https://img.shields.io/badge/Tauri-2.x-orange)
![License](https://img.shields.io/badge/license-MIT-green)

## ✨ 功能特性

### 实时监控

- 🖥️ **CPU** — 总使用率 + 每核心使用率
- 🧠 **内存** — 已用 / 总量 / 压力状态
- 💾 **磁盘** — 各分区容量与使用率 + 读写速率（IOKit 块设备计数按刷新窗口取增量，已与 `iostat` 逐帧对账；拿不到计数的平台显示 `—`；容量都读不出来的挂载点整行显示 `—`，不会画一根 0 % 的进度条）
- 🌐 **网络** — 分接口实时上下行速率 + 接口表（状态 / IPv4 / IPv6 / MAC）；后端同时采集累计字节与包数，界面暂未展示
- 🔬 **迷你模式** — 顶栏按钮或 `m` 键把界面收成一屏关键指标（CPU 含核数 / 内存含后端压力档 / 磁盘含挂载点与剩余 / 网络两接口之和 / 运行时长 + 两根迷你趋势线 + 一行告警状态），`Esc` 退出；900×600 与 560×420 两档实测无横/纵向滚动。进入时**停掉进程枚举流**，整屏**零新增 IPC**（数值全来自概览页那同一帧快照与同一段历史，"看哪块盘"与趋势窗口都复用概览的口径函数）；读数颜色按后端回过的 warning 档上色，越限文案只来自后端推来的告警事件，无首帧时一律 `—`

### 系统清理

- 🗑️ **垃圾文件扫描** — 白名单目录逐类统计体积：macOS（`~/Library/Caches`、`~/Library/Logs`、Xcode DerivedData、模拟器缓存、系统临时目录）、Linux（`~/.cache`、`~/.local/share/Trash`、`/tmp`）、Windows（`%WINDIR%\Temp`、Recent、`C:\Windows\Logs`）
- 📈 **进度与取消** — 逐目录推送真实进度（已扫 N / 共 M 个目录 + 当前目录文件数与累计体积），随时可取消；取消后保留已扫出的部分并在报告里明示"结果不完整"，不谎报完整
- 🔍 **风险分级** — 安全 / 中等 / 高风险 三档提示
- ✅ **选择性清理** — 扫描结果逐类勾选（表格 `rowSelection`），只清被选中的类别 id
- ⚠️ **清理前确认对话框** — 列明类别、体积与待清理目录清单，并明示删除不可恢复（后端逐个 `fs::remove_file`，不走回收站）
- 🛡️ **路径守卫** — 清理 id 必须属于扫描白名单；每个目录 `canonicalize` 后过黑名单（系统目录、凭据目录）与允许根，不通过即跳过并计入 `skippedPaths`
- 📊 **清理报告** — 显示释放空间、删除文件数、失败数与跳过数

### 大文件扫描

- 📏 **自定义阈值** — 按 MB 设置最小文件大小
- 📂 **用户目录遍历** — 默认从 `~` 起（深度上限 12），返回前 50 条
- 📅 **排序展示** — 后端按体积降序取前 50 条；表头可按大小再排序，修改时间仅作展示

### 启动项管理

- 🚀 **跨平台支持** — macOS Login Items（`osascript`）/ LaunchAgents plist、Linux XDG autostart / systemd user 目录、Windows 当前用户注册表 `HKCU\...\CurrentVersion\Run`（`reg query`，不含机器级启动项）
- ⚙️ **状态查看** — 显示每个启动项的来源、命令路径
- 🚫 **禁用 / 删除 / 恢复** — 只操作文件型来源（macOS `~/Library/LaunchAgents/*.plist`、Linux `~/.config/autostart/*.desktop`）：每一项都要**逐字输入启动项名称**确认，执行是一次 `fs::rename` 把原件移进应用数据目录下的 `startup-backup/`（删除再深一层 `out/`），**不做物理删除**，禁用项仍在列表里可一键恢复。需要提权或没有文件可移的来源（Login Items 自动化、Windows 注册表、systemd 用户单元）照旧枚举，但标 `operable=false`，界面只给"不支持在此操作"标签而不给按钮

### 网络工具

- 🔄 **DNS 刷新** — 一键执行平台用户态命令（macOS `dscacheutil -flushcache`、Linux `resolvectl flush-caches`、Windows `ipconfig /flushdns`）；需要提权时后端只返回 `manualCommand` 供用户自行执行，**应用内永不调用 `sudo`**
- 📡 **接口信息** — 查看所有网络接口、IPv4/IPv6、MAC（Windows 下 MAC 暂未采集）

### 进程管理

- 📋 进程列表（PID / 名称 / CPU / 内存 / 运行时长；线程数在 macOS 无来源，显示 `—`）
- 🔎 **服务端搜索 / 排序 / 分页** — 关键字与排序下发到后端执行，每帧只回当页数据（默认 20 行/页，可选 20 / 50 / 100 / 200 / 300；300 即后端 `MAX_PROCESSES_PER_PAGE`，实测全量命中 717 进程时不再整表推送）
- ⚡ **进程表虚拟滚动** — 当页超过 50 行时只渲染可视窗口：实测 500 行数据 → DOM 恒 **15 行**、卡片内节点 **351**（整页渲染 50 行时是 906），首帧 **42 / 44 ms**（< 100 ms 目标）。注意该组时间来自 headless Chrome 挂载真实 `ProcessTab`，WKWebView 内未复测；后端一页最多 300 条，所以真机 700 进程是"300 行/页 + 翻页"而不是单页 500 行
- 🪟 **进程详情抽屉** — PID / 状态 / 父进程（可跳转）/ CPU / 内存 / 运行时长 / 启动时刻 / 可执行路径 / 工作目录；按隐私约束**不采集命令行参数与环境变量**
- 🛑 **结束进程** — 跨平台 SIGTERM→宽限→SIGKILL / `taskkill`，必经 `validate_kill`（PID ≤ 100、系统关键进程名单、属主校验硬拒），确认框内 `critical` 级别需键入进程名

### 系统信息

- 🖥️ 主机名、操作系统版本、内核
- ⏱️ 系统运行时间
- 💻 CPU 型号与核心数（含每核心实时占用）
- 💾 总内存 / 已用内存 / 压力状态

### 趋势图表

- 📈 CPU / 内存使用趋势图，**60 秒 / 5 分钟 / 1 小时** 三档可切换（超采样区间按连续区段取均值降采样）
- 💾 趋势**跨重启**：后端按 10 s 落盘、保留 7 天，前端挂载时回填 1 小时并如实标注"含 N 个落盘历史点（10 s 一点）"还是"仅本次会话实时采集"（存储写不进时直接报错，不显示空曲线冒充正常）
- 📤 趋势可**导出 CSV**：概览趋势行上的"导出 CSV"按**当前显示的范围**导出，列为 `timestamp_ms / cpu_percent / memory_percent / rx_bytes_per_sec / tx_bytes_per_sec / bucket_seconds`；**每行自带桶宽**（10 = 原始采样点，更大 = 该桶均值），否则二次计算的人会拿分钟均值当瞬时值。文件里全是指势数值，不含路径/挂载点/进程名；超过体积上限时**拒并提示缩小范围**，不会静默截断成一份"看起来完整"的半份文件
- 📊 实时滚动更新
- 🎨 颜色阈值警告（正常 / 警告 / 危险）

### 资源告警

- 🚨 CPU / 内存 / 磁盘各有一对 `warning` / `critical` 阈值（默认 80/95、85/95、90/95），阈值在侧边抽屉里改、`localStorage` 持久化后下发后端
- 🧮 **判定在后端的采集循环里执行**：连续 N 帧越限（默认 3）才触发，只抖一下不报；同一 `(指标, 目标, 级别)` 有冷却窗口（默认 60 s）不重复推送
- 💾 磁盘告警锁定"可用分区里最满的那一块"，事件里带真实挂载点 —— 不会给一个不知道是哪块盘的"磁盘 90 %"
- 🔕 触发时三路出口：应用内 Toast（critical 红色 / warning 黄色）、抽屉内的**落盘告警历史**列表、**操作系统的通知中心**（T5-02）。后端阈值会夹取脏输入并把真正生效的值回给前端覆盖，界面不会显示一份"后端其实没接受"的配置
- 🔔 系统通知这一路只报事实：告警抽屉里显示"已提交 N 条 · 失败 M 条"和最后一次失败原文，**不写"已送达"也不写"已授权"** —— 插件在桌面端读不到系统真实授权状态（`permission_state()` 是恒 `Granted` 的桩），macOS 的默认通知后端又是在句柄析构时才发送、并把错误直接丢掉，所以 `Ok` 只代表"交给了通知接口"。看不到横幅就先按抽屉里那颗"发一条测试通知"自己确认；通知与窗口内 Toast 同进同退，共用那个手动总开关与 60 秒冷却，没有第二条去重路径
- 🗂 告警记录**跨重启**：每条触发写进 `<app_data_dir>/alerts/alerts.jsonl`（JSON Lines，上限 5 000 条 / 保留 30 天），重开应用即回填，列表里以 `回填` 标签区分来源；写在盘上的 `target` 过一遍脱敏，会话内的实时事件仍显示真实路径
- 📍 告警时刻会标在 CPU / 内存趋势图上（竖线，同一时间桶多条合成一根）：**只贴真实采样点**，落在当前窗口外的不画、只写"另有 N 条未画"，对齐误差按实际值标在图注里

### 诊断助手（只建议，不执行）

- 💬 一个顶层页签：用中文问"哪个进程占 CPU 最高？""为什么这么卡？""磁盘还剩多少空间"，它读**与界面同一份**采集快照和**同一套**告警阈值给结论，不接任何外部模型
- 🧠 识别 9 类意图（进程排行 / 综合诊断 / 内存压力 / 磁盘空间 / 网络吞吐 / 启动项 / 垃圾清理 / 温度风扇 / 未识别），判定全在后端 `src-tauri/src/agent.rs` 的纯函数里完成
- 🧭 **它能做的只有两件事**：打开某个页签、把某个 PID 填进进程表关键字。卡片上没有"执行"按钮，`已执行：否` 是常量而不是状态；终止进程、清理文件、刷新 DNS 都必须去对应页面自己确认、走各自的校验流程
- 🚫 像"帮我 kill 掉它"这类自由文本会被**拒绝**并只回一句"去对应页面，那里有确认步骤"，不生成任何命令文本；建议的字段类型里根本没有放命令、参数或路径的位置（`Suggestion` 枚举只有两种导航模板）
- 🧪 采集不到就明说"没有数据"，不颁一个 0 % 的冠军、不编一个数；温度/风扇读的是 `get_thermal` 的真实报告 —— 读到就按**硬件自己上报的上限**排名，读不到（macOS/Windows 没有免提权通路）就只回答"为什么没有"并且不给建议，两条路都不会补一个估算值
- ⚙️ 只有真正需要排行榜的三类问题才付一次性采集的约 260 ms（走 `spawn_blocking`，不占 IPC 线程），问磁盘空间不会触发全机器进程枚举

### 用户体验

- 🌓 亮色 / 暗色主题切换
- ⚡ 刷新频率可选（0.5 / 1 / 2 / 5 秒），动态下发到后端采集循环
- 🧲 偏好持久化：主题、刷新频率、趋势区间、当前 Tab、告警阈值共 5 项（`localStorage`，键前缀 `z-biz-sys:v1:`），陌生/旧版本残留值会被白名单回落而不是把界面打成空白
- 📤 偏好导入 / 导出（入口在"系统信息"页底部）：导出为一枚 JSON（只含上面那 5 项，**不含路径、进程或主机信息**）；导入是"先逐项预览、再点应用"两步，每行标 `采纳 / 已夹取 / 文件没有 / 值不可用`，脏值最多回落默认值、绝不把界面清空，文件里本版本不认的键只报告不写入。后端只守落盘边界：必须 `.json`、只能在用户主目录或临时目录（含 `/tmp`）之下、拒受保护目录、上限 64 KiB、写用 `.tmp` + `rename` 原子覆盖；认不出 `app` 标识或 `schemaVersion` 比本版更高一律拒收（不做猜测式迁移）
- 🧱 每个 Tab 独立 `ErrorBoundary`：某一页渲染抛错只塌该页，其余页与顶栏存活并可重试
- 🔔 后端 `AppError.code` 决定提示级别：受保护路径被拦、平台不支持 → 蓝色 info；进程已退出 / 路径不存在 → 黄色 warning（并顺带刷新对应列表、收起读不到内容的抽屉）；需要提权 → warning 且只给可复制的手动命令；参数不合法、命令失败、IPC 不通 → 红色 error
- 📱 窗口自适应

---

## ⚠️ 已知限制（与实现一致，不是待办口号）

| 项 | 现状 |
|---|---|
| 启动项禁用 / 删除 | 已实现（T3-08），但边界要说清：① 覆盖范围只有文件型来源，Windows 无免提权通路（注册表项 `operable=false`）、Login Items 与 systemd 用户单元同理不给按钮；② "禁用"是把 plist 移出启动目录，**不改** `Disabled` 键，也不触碰 `launchctl`；③ "删除"仍保留字节在 `startup-backup/<来源>/out/`，需要用户自行清盘；④ 移动逻辑在单测里用临时目录跑过 22 条（含确认闸门、不覆盖、符号链接邻居、白名单），**真机 GUI 上点一次"禁用"仍未验过** |
| 启动项链接判定 | 指向同目录另一份 plist 的符号链接会被拒（曾是一个真 bug：canonical 之后才判"普通文件"，于是搬走的是邻居的真身并改名成链接名）。现在两处判定都在解析前，见 04 的"启动项操作" |
| macOS 回收站 | 清理白名单**不含** `~/.Trash`（Linux 侧才有 `~/.local/share/Trash` 项）；macOS 可清理类别为缓存/日志/Xcode/模拟器/临时目录 |
| 磁盘读写速率 | 有来源（sysinfo 0.33.1 `Disk::usage()` 增量 ÷ 刷新窗口，按挂载点缓存、窗口内各帧沿用同一值）。取不到块设备计数的平台返回 `None`、界面显示 `—`；真·空闲显示 `0 B/s` 而不是 `—`；首个刷新窗口之前一律 `—`，不拿 0 冒充空闲。同一物理盘的多个 APFS 卷会显示相同计数 |
| 读不出容量的分区 | 后端 `available: false`（`total_bytes == 0` 的判据），`usage_percent` 因字段类型是 `f64` 而保持 `0.0`；界面看 `available` 把容量/已用/可用/使用率四列统一显示 `—`。这是 FI-04 那轮补测时查出来的显示缺陷：修之前一根 0 % 的进度条会把"断掉的挂载点"说成"一块全空的盘" |
| 进程线程数 | 无数据来源 —— sysinfo 的线程数 API 仅 Linux 有值，后端字段固定 `None`、界面显示 `—`，不造 0 值 |
| 趋势历史 | 已落盘：`<app_data_dir>/history/points.jsonl`（JSON Lines，10 s 一点、保留 7 天、时钟回拨丢点不补假点，实测 99.7 B/点 ⇒ 满额 ≈6.0 MB），重启后自动回填并标注来源与分辨率。**未落盘**的是磁盘 I/O 与进程历史 —— 前者只在实时链路里给窗口均值、没有跨重启序列，后者是按需查询的分页快照而非时间序列 |
| 资源占用 | 实测 release 常驻内存 94.5–122.9 MB（含 WKWebView），未达"常驻 < 80 MB"的早期目标；采集 CPU 开销实测 0.77 %–1.52 %（1 Hz，< 2 % 达标）。2026-09-22 24:13 用带温度/风扇与系统通知的 release 包重测：主进程 RSS t+45s/t+70s 稳定在 98 816 / 98 848 KB（≈101.2 MB，不增长），`ps %cpu` 0.4–0.5 %。归属说明：WebKit 的 GPU/Networking/WebContent 是 `ppid=1` 的 XPC Service，`ps` 按父子关系收不到，所以"含 WebView 的总额"没有严格算法 —— 但主进程自身就已越过 80 MB，结论不受影响 |
| 告警历史 | 已落盘：`<app_data_dir>/alerts/alerts.jsonl`（上限 5 000 条、修剪到 90 % 低水位、保留 30 天，实测 ≈144.5 B/条）。可查询窗口最大到保留期（默认展示 7 天），**单次返回最多 300 条**——列表会照实写"只列出最近 300 条，另有 N 条更早的未列"，N 是窗口内的真实条数而非估算。与趋势点不同的一条口径：时钟回拨时趋势点直接丢（图上留缺口），告警记录一条不丢、由查询侧重排 |
| 前端包体 | 单 chunk 1,585.37 kB / gzip 491.50 kB（实测 `dist/assets/index-D_ELq_sU.js` 1 585 365 B；温度/风扇收尾 1 581 950 B → 系统通知 +2 097 B → 历史 CSV 导出 +1 079 B → 全屏自动静默 +239 B，CSS 1 441 B 未动），尚未做代码分割（不在规划任务里） |
| DNS 刷新 | 只执行用户态命令；平台要求提权时返回 `manualCommand` 由用户自行执行，应用内零 `sudo` |
| 告警 | 引擎、阈值配置、**落盘历史**、**系统通知**与**全屏自动静默**都已落地（判定在后端采集循环，见上"资源告警"）。系统通知**没有扩任何权限**：投递走 `tauri-plugin-notification` 的 Rust API，`capabilities` 仍是那 8 条显式权限、无通配项，并由回归逐字钉住。**未做**：演示/全屏自动静默（只有手动总开关）、磁盘趋势图上的告警标注（磁盘告警只写在磁盘相关界面，概览两张图没有磁盘曲线） |
| 温度 / 风扇 | 已落地（T5-08，`src-tauri/src/platform/thermal.rs`）：**Linux 真读** `/sys/class/hwmon`（`tempN_input` 毫摄氏度、`fanN_input` RPM、上限只取硬件自己上报的 `tempN_max`），hwmon 全空才退 `thermal_zone*`。**macOS / Windows 如实缺测**：`get_thermal` 返回 `{sensors: [], reason: "…"}`，界面显示 0 行 + 那句原因，诊断助手同样只回原因、不给建议 —— 本机实测 `powermetrics --samplers smc` 回 `unrecognized sampler: smc`、`sysctl` 只有 IPv6 的 tempaddr 键、`pmset -g therm` 三行 `No … recorded`、`ioreg` 里 30 处 "temperature" 全是类名，剩下的私有 SMC 通路要 root ⇒ 与"应用内零 `sudo`"冲突，所以不做，也不会用机型平均值或估算值补位。判级（正常/偏高/越过硬件上限/未上报上限）只在后端一处算，界面只上色；**风扇 0 RPM 显示"停转"而不是缺测** |
| 发布物体积 | 最近一次 `npm run tauri build`（2026-09-23 02:42，含 T5-08 温度、T5-02 系统通知、历史 CSV 导出、全屏自动静默）：`.app` 内二进制 **13 937 568 B**、`.app` 目录 **15 634 432 B**、`.dmg` **7 679 660 B**。对比 T5-07 那轮的 13 425 248 B / 7 511 750 B ⇒ 二进制累计长 **512 320 B**、dmg 长 **167 910 B**（其中 CSV + 全屏静默这一轮 +40 560 B / +4 421 B） |
| 迷你模式的边界 | ✅ 已落地（T5-07），但**只做同窗口紧凑视图**：方案原句里的"缩小窗口"没做 —— `capabilities` 是 8 条显式权限且**不含任何窗口写类权限**（`core:default` 只给只读 getter），JS 侧拿不到 `set_size`/`hide`；原生菜单栏常驻还要加 `tray-icon` feature。两者都要扩权限，属"先问再动"，与系统通知同一道门槛。`miniMode` 也刻意**不进**偏好快照（会话级，重启不保留） |
| 自然语言 Agent | ✅ 已落地（T5-04/05/06），但**刻意窄于"Agent"这个词能撑开的范围**：一问一答、无多轮上下文、对话不落盘（因此也不存在"历史里带路径/进程名"的脱敏问题）、无任何执行能力。它读的是界面上已有的采集快照，不额外去读命令行参数或环境变量 |
| 每进程 CPU 的一次性采集 | 曾有缺陷并已修复（H-06，2026-09-22）：冷 `System` 在 macOS 上要**三次**触碰才有真实 CPU 差值，旧的一次性路径只有两次 ⇒ 排行结果等于"谁先被枚举到"。现由 `collect_processes_warmed` 收口、`per_process_cpu_is_actually_sampled` 回归 |
| 跨平台验证 | 代码含 macOS / Linux / Windows 三分支，但**只有 macOS（Apple M3）真机实测**过；Windows 注册表启动项、Linux XDG/systemd 分支未经真机核对（对应方案 T5-09 未做） |

---

## 🛠 技术栈

| 层 | 技术 |
|---|---|
| **桌面框架** | [Tauri 2.x](https://tauri.app/) (Rust + WebView) |
| **前端** | React 19 + TypeScript + Vite 6 |
| **UI 组件** | [Ant Design 6](https://ant.design/) |
| **图表库** | [Recharts](https://recharts.org/) |
| **系统监控** | [sysinfo](https://github.com/GuillaumeGomez/sysinfo) (Rust) |
| **状态管理** | React hooks + Tauri 事件流（`src/hooks/useSystemMonitor.ts`、`useProcessStream.ts`、`usePrefs.ts`、`useProcessQuery.ts`、`useAlerts.ts`），无常驻状态库；阈值/告警判定在后端 `src-tauri/src/alert.rs` |

---

## 🚀 开发

### 前置依赖

- Node.js 22+
- Rust stable（通过 [rustup](https://rustup.rs/) 安装）
- Tauri CLI: `cargo install tauri-cli --version "^2.0.0"`

### 启动开发服务器

```bash
# 1. 安装前端依赖
npm install

# 2. 启动 Tauri 开发模式（带热重载）
npm run tauri dev
```

应用窗口会自动启动，系统监控默认每 1 秒采集一帧（顶栏可切 0.5 / 1 / 2 / 5 秒，选择会持久化）。

### 构建发布版本

```bash
# 本地构建当前平台
npm run tauri build

# 产物路径
src-tauri/target/release/bundle/
├── dmg/      # macOS
├── msi/      # Windows
├── deb/      # Debian / Ubuntu
└── AppImage/ # Linux 通用
```

---

## 🔁 持续集成

[`.github/workflows/ci.yml`](.github/workflows/ci.yml)（T5-09）在 **PR 与手动触发**上跑三条腿门禁：`macos-latest` / `ubuntu-22.04` / `windows-latest`，步骤为 Linux apt 依赖 → `npm ci` → `tsc --noEmit` → `npm run build` → `cargo test --lib` → `cargo clippy --all-targets`；macOS 腿再补一条 `cargo check --target x86_64-apple-darwin` 作为 Intel 的编译门禁（用交叉 check 而不是 Intel runner）。

- 刻意**不**监听 `push`：本仓有直接往 main 推 WIP 的习惯，加 push 触发会让每次 WIP 都占满三条 runner。
- clippy 刻意**不带 `-D warnings`**：只有 macOS 一侧实测过 0 warning；先把 Linux/Windows 的真实 warning 清单收回来，再决定要不要升成门禁。
- `npm run build` 必须排在任何 cargo 步骤之前：`dist` 在 `.gitignore` 里，而 `tauri::generate_context!()` 编译期展开时见 `frontendDist` 目录缺失直接 panic。
- 发布链 [`.github/workflows/build.yml`](.github/workflows/build.yml)（4 平台产物 + Release）独立存在，本项未改它。
- 状态如实：矩阵文件已建、本地可验部分已验（两份 workflow 解析通过、`npm ci --dry-run` exit 0、workflow 里的命令行原样跑绿 `cargo test --lib` **92/92** 与 clippy **0 warning**），但**第一次运行还没触发** —— push 之后 `gh workflow run ci.yml` 跑第一轮。

---

## 📦 发布流程

本项目使用 `scripts/release.sh` 自动化发布：

```bash
# 1. 提交所有未提交改动
git add -A
git commit -m "feat: your changes"

# 2. 触发发布流程
bash scripts/release.sh --yes
```

脚本会：

1. 📌 将版本号 `0.1.0` → `0.2.0`（minor +1，patch 归零）
2. 🔄 同步更新 `package.json`、`Cargo.toml`、`tauri.conf.json`
3. 📤 推送 main 分支
4. 🏷️ 创建 `v0.2.0` tag 并 push
5. 🚀 GitHub Actions 自动构建 4 个平台安装包并创建 Release

支持的参数：

```bash
bash scripts/release.sh            # 交互式确认每一步
bash scripts/release.sh --yes      # 全自动（推荐 CI 使用）
bash scripts/release.sh --dry-run  # 仅预览计划，不实际执行
bash scripts/release.sh --help     # 查看帮助
```

---

## 🤝 贡献

欢迎贡献代码、报告 Bug 或提出功能建议！

1. Fork 本仓库
2. 创建 feature 分支：`git checkout -b feat/your-feature`
3. 提交改动：`git commit -m "feat: add your feature"`
4. 推送分支：`git push origin feat/your-feature`
5. 创建 Pull Request

---

## 📄 许可证

[MIT](./LICENSE) © z-biz-tool

---

## 🔗 相关项目

- [z-biz-tool-box](https://github.com/z-biz-tool/z-biz-tool-box) — 插件化工具箱（32+ 内嵌工具）
- [z-biz-tool-db](https://github.com/z-biz-tool/z-biz-tool-db) — 数据库 GUI
- [z-biz-tool-note](https://github.com/z-biz-tool/z-biz-tool-note) — 笔记工具
- [z-biz-tool-file](https://github.com/z-biz-tool/z-biz-tool-file) — 文件管理
- [z-biz-tool-terminal](https://github.com/z-biz-tool/z-biz-tool-terminal) — 终端工具
- 完整列表见 [z-biz-tool 组织主页](https://github.com/z-biz-tool)