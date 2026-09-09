# z-biz-tool-sys

> 一款轻量、实时的跨平台系统监控仪表盘，参考 [iStat Menus](https://bjango.com/istatmenus/) 和 [Stats](https://github.com/exelban/stats) 的产品形态，基于 Tauri 打造。

![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Windows%20%7C%20Linux-blue)
![Tech](https://img.shields.io/badge/Tauri-2.x-orange)
![License](https://img.shields.io/badge/license-MIT-green)

## ✨ 功能特性

### 实时监控

- 🖥️ **CPU** — 总使用率 + 每核心使用率
- 🧠 **内存** — 已用 / 总量 / 压力状态
- 💾 **磁盘** — 各分区使用情况、读写字节数
- 🌐 **网络** — 实时上下行速度、累计流量

### 系统清理 🆕

- 🗑️ **垃圾文件扫描** — 自动识别系统缓存、临时文件、回收站等
- 🔍 **风险分级** — 安全 / 中等 / 高风险 三档提示
- ✅ **选择性清理** — 勾选要清理的类别，支持全选 / 反选
- 📊 **清理报告** — 显示释放空间、删除文件数

### 大文件扫描 🆕

- 📏 **自定义阈值** — 按 MB 设置最小文件大小
- 📂 **全盘扫描** — 扫描整个用户目录
- 📅 **排序展示** — 按大小 / 修改时间排序

### 启动项管理 🆕

- 🚀 **跨平台支持** — macOS Login Items / LaunchAgent、Linux XDG / systemd、Windows 注册表
- ⚙️ **状态查看** — 显示每个启动项的来源、命令路径
- 🚫 **禁用 / 删除** — 一键管理启动项（待加固）

### 网络工具 🆕

- 🔄 **DNS 刷新** — 一键刷新系统 DNS 缓存（macOS / Linux / Windows）
- 📡 **接口信息** — 查看所有网络接口、IP、MAC

### 进程管理

- 📋 进程列表（PID / 名称 / CPU / 内存 / 线程数）
- 🔄 按 CPU 或内存排序
- 🛑 **一键结束进程** — 跨平台 kill / taskkill

### 系统信息

- 🖥️ 主机名、操作系统版本、内核
- ⏱️ 系统运行时间
- 💻 CPU 型号与核心数
- 💾 总内存 / 已用内存

### 趋势图表

- 📈 CPU / 内存使用趋势图（最近 60 秒）
- 📊 实时滚动更新
- 🎨 颜色阈值警告（正常 / 警告 / 危险）

### 用户体验

- 🌓 亮色 / 暗色主题切换
- ⚡ 极低资源占用（Tauri 优势）
- 📱 窗口自适应

---

## 🛠 技术栈

| 层 | 技术 |
|---|---|
| **桌面框架** | [Tauri 2.x](https://tauri.app/) (Rust + WebView) |
| **前端** | React 19 + TypeScript + Vite 6 |
| **UI 组件** | [Ant Design 6](https://ant.design/) |
| **图表库** | [Recharts](https://recharts.org/) |
| **系统监控** | [sysinfo](https://github.com/GuillaumeGomez/sysinfo) (Rust) |
| **状态管理** | Zustand |

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

应用窗口会自动启动，系统监控数据每 1 秒刷新。

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