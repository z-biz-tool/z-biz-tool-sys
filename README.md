# z-biz-tool-sys

> 一款轻量、实时的跨平台系统监控仪表盘，参考 [iStat Menus](https://bjango.com/istatmenus/) 和 [Stats](https://github.com/exelban/stats) 的产品形态，基于 Tauri 打造。

![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Windows%20%7C%20Linux-blue)
![Tech](https://img.shields.io/badge/Tauri-2.x-orange)
![License](https://img.shields.io/badge/license-MIT-green)

## ✨ 功能特性

### 实时监控

- 🖥️ **CPU** — 总使用率 + 每核心使用率
- 🧠 **内存** — 已用 / 总量 / 压力状态
- 💾 **磁盘** — 各分区容量与使用率（读写速率依赖 sysinfo 升级，暂未采集）
- 🌐 **网络** — 分接口实时上下行速率 + 接口表（状态 / IPv4 / IPv6 / MAC）；后端同时采集累计字节与包数，界面暂未展示

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
- 🚫 **禁用 / 删除** — 尚未实现，界面按钮已置灰并标注原因（会移动/删除用户真实文件，需单独授权后才做）

### 网络工具

- 🔄 **DNS 刷新** — 一键执行平台用户态命令（macOS `dscacheutil -flushcache`、Linux `resolvectl flush-caches`、Windows `ipconfig /flushdns`）；需要提权时后端只返回 `manualCommand` 供用户自行执行，**应用内永不调用 `sudo`**
- 📡 **接口信息** — 查看所有网络接口、IPv4/IPv6、MAC（Windows 下 MAC 暂未采集）

### 进程管理

- 📋 进程列表（PID / 名称 / CPU / 内存 / 运行时长；线程数依赖 sysinfo 升级，暂未采集）
- 🔎 **服务端搜索 / 排序 / 分页** — 关键字与排序下发到后端执行，每帧只回当页 20 行（实测全量命中 717 进程时不再整表推送）
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
- 📊 实时滚动更新
- 🎨 颜色阈值警告（正常 / 警告 / 危险）

### 用户体验

- 🌓 亮色 / 暗色主题切换
- ⚡ 刷新频率可选（0.5 / 1 / 2 / 5 秒），动态下发到后端采集循环
- 🧲 偏好持久化：主题、刷新频率、趋势区间、当前 Tab（`localStorage`，键前缀 `z-biz-sys:v1:`），陌生/旧版本残留值会被白名单回落而不是把界面打成空白
- 🧱 每个 Tab 独立 `ErrorBoundary`：某一页渲染抛错只塌该页，其余页与顶栏存活并可重试
- 📱 窗口自适应

---

## ⚠️ 已知限制（与实现一致，不是待办口号）

| 项 | 现状 |
|---|---|
| 启动项禁用 / 删除 | 未实现，按钮 `disabled` + tooltip 说明。原因：要移动/删除用户真实 `~/Library/LaunchAgents` 等文件，属不可逆操作 |
| macOS 回收站 | 清理白名单**不含** `~/.Trash`（Linux 侧才有 `~/.local/share/Trash` 项）；macOS 可清理类别为缓存/日志/Xcode/模拟器/临时目录 |
| 磁盘读写速率、进程线程数 | 无数据来源（sysinfo 保持 0.30.13 未升级），后端字段返回 `None`、界面显示 `—`，不造 0 值 |
| 趋势历史 | 已落盘：`<app_data_dir>/history/points.jsonl`（JSON Lines，10 s 一点、保留 7 天、时钟回拨丢点不补假点，实测 99.7 B/点 ⇒ 满额 ≈6.0 MB），重启后自动回填并标注来源与分辨率。**未落盘**的是磁盘 I/O 与进程历史 —— 前者无数据来源，后者是按需查询的分页快照而非时间序列 |
| 资源占用 | 实测 release 常驻内存 94.5–122.9 MB（含 WKWebView），未达"常驻 < 80 MB"的早期目标；采集 CPU 开销实测 0.77 %–1.52 %（1 Hz，< 2 % 达标） |
| 前端包体 | 单 chunk 1,535.21 kB / gzip 474.33 kB，尚未做代码分割 |
| DNS 刷新 | 只执行用户态命令；平台要求提权时返回 `manualCommand` 由用户自行执行，应用内零 `sudo` |
| 告警引擎 / 温度风扇 / 菜单栏迷你模式 / 自然语言 Agent | 未开始（Phase 5 规划中） |
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
| **状态管理** | React hooks + Tauri 事件流（`src/hooks/useSystemMonitor.ts`、`src/hooks/useProcessStream.ts`），无常驻状态库 |

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