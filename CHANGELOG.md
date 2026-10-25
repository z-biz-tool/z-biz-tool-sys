# Changelog

z-biz-tool-sys 版本变更记录。格式遵循 [Conventional Commits](https://www.conventionalcommits.org/)。

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
