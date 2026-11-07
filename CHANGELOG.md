# Changelog

z-biz-tool-sys 版本变更记录。格式遵循 [Conventional Commits](https://www.conventionalcommits.org/)。

## [已废弃] - 2026-11-06

**本仓废弃，能力并入 z-biz-tool-file。**

系统监控成为 z-biz-tool-file 的第五个板块（活动栏最下面那个「系统监控」图标，
在「数据库」下方），独立 app 不再出包、不再单独发版，能力只维护一份。

搬过去的东西：

- 后端 16301 行 Rust（19 个模块：sysinfo 采集、IOKit 块设备对账、进程枚举与归因、
  清理白名单与路径守卫、启动项备份、告警引擎、历史落盘、通知投递、Agent）
  → `z-biz-tool-file/src-tauri/src/sys_app/`
- 前端 7798 行（44 文件：8 个 tab + 11 个面板 + 7 个 hooks + 契约与 i18n）
  → `z-biz-tool-file/src/apps/sys/`
- 33 个 Tauri 命令全部注册。唯一没搬的是 `zbiz_take_pending`：它是家族统一深链
  命令，z-biz-tool-file 那边本来就有一份同名同义的，重复注册会让 Tauri 启动时 panic。

合并时做的改动（与本仓原实现有意的差异）：

- **采集改为按需启停**。本仓是独立 app，「进程活着 = 用户在看监控」，采集循环
  无条件跑是对的。并进 file 后不同了：十次启动有九次是为了找文件，却每次都在
  后台每秒 refresh 一遍 CPU/内存/磁盘/网络（sysinfo 在 macOS 上要走 sysctl +
  statfs + IOKit）。现在 `MonitorService` 有一个 `active` 闸门（默认 false），
  `SysApp` 挂载时置真、卸载时置假。注意它和用户的「暂停」是两件事：`paused`
  是界面可见的状态，`active` 只是板块在不在前台。
- **深链改走交接槽**。合并前各 app 注册一个 `zbiz://` 接收器没问题；合并进同一
  个 webview 后，共享层的接收器是**取走式**的（`take_pending` 会清空队列），
  两个接收器互相抢 → 表现为「深链点了没反应」。现在只留 file 的接收器，
  按 `item.product` 分流到 `src/apps/sys/handoff.ts`（与终端板块同一套模式）。
- **主题跟随 file**。本仓自带 `ConfigProvider` + 自己的 `darkMode` 偏好。
  现在首次进入板块时读 file 的 `z-tool-theme` 作为初值（之后仍由本板块自己的
  偏好管，因为那条偏好要跟着偏好文件导入导出走）。
- **提示通道统一到 antd `<App>` context**。本仓原先用 `message.useMessage()`
  自己搭 holder，静态 API 吃不到主题；现在整块换成 `App.useApp()`。
- **数据目录换了**。历史趋势与告警历史落在 file 的 `app_data_dir` 下，
  旧 sys app 采过的历史不会自动迁移（滚动数据，保留窗口 7 天，重采即可）。
- **首屏按需加载**。`SysApp` 走 `lazy()`（102KB 独立 chunk），
  板块内部的 recharts 也二次 lazy —— 首屏 1447KB 里不含这部分。

本仓转为只读历史存档。**功能改动请去 z-biz-tool-file。**
`latest.json` 保留为 0.2.0 不动（版本号不涨就不会推假更新），
但已加 `_deprecated` 说明字段，防止有人误用它发版。

## [0.2.0] - 2026-10-25

自 v0.1.0 以来共 20 个提交：3 个 feat、8 个 fix。进程观测从「能看」到「能归因」（运行时归类 / 脱离启动者 / 监听端口反查三块视图），并修掉 Linux 属主列静默全错这类真缺陷。版本账自 8 月停更，本次对齐代码实际成熟度（详见 lead `doc/分发/版本账失序清单.md`）。

### Features

- 进程观测补三块视图：运行时归类、脱离启动者、监听端口反查
- 闭合 T3-02 per-interface 曲线 + T4-09 title 本地化
- 收编 T-058 安全加固 + cargo target 产物探测

### Fixes

- Linux 上属主列静默全错 —— ProcessRefreshKind 从未请求 user
- 迷你菜单栏逐帧重算收进 useMemo，尖峰不再被均值抹平
- 修「装了个寂寞还报喜」——安装成功判定改为可测代码
- 产物路径不再硬编码 src-tauri/target，改由 cargo metadata 解析 target_directory
- 真采集用例的环境依赖判据换成值对应关系（不再靠环境猜）
- 四处「只在 macOS 成立」的前提改成按平台表达同一件事
- clippy 收严后暴露的平台专属告警清零 + 修一条偶发红的属主对账用例
- 图标挂进 build script 的重跑指纹

### Chore / CI / Docs

- MIT 许可证统一；换家族矢量图标源
- rustfmt 365 处清零并把 fmt 变成门禁，clippy 同步收严
- 逐条核对 05 任务表 42 行，查出 2 处「抬头说全收口、标准没达到」并记账
- release.sh 从 lead 模板下发（--bump 档位 + 前置门禁 + 版本基准取最高 tag）
