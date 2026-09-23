import { useCallback, useEffect, useState } from "react";
import {
  Alert,
  Badge,
  Button,
  ConfigProvider,
  Layout,
  Popconfirm,
  Select,
  Space,
  Switch,
  Tabs,
  Tooltip,
  Typography,
  message,
  theme,
} from "antd";
import {
  BellOutlined,
  ClearOutlined,
  CompressOutlined,
  DashboardOutlined,
  FileSearchOutlined,
  RocketOutlined,
  RobotOutlined,
  SyncOutlined,
} from "@ant-design/icons";
import dayjs from "dayjs";
import { invoke } from "@tauri-apps/api/core";
import { save as saveDialog } from "@tauri-apps/plugin-dialog";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  Commands,
  MonitorEvent,
  type AgentReply,
  type AlertEvent,
  type CleanupResult,
  type DnsFlushResult,
  type JunkReport,
  type KillValidation,
  type LargeFile,
  type ProcessDetail,
  type ScanProgress,
  type StartupItem,
  type StartupAction,
  type StartupOutcome,
  type CsvExportOutcome,
  type ThermalReport,
} from "./ipc_contract";
import { useAlerts } from "./hooks/useAlerts";
import { usePrefs } from "./hooks/usePrefs";
import { useProcessQuery } from "./hooks/useProcessQuery";
import { useProcessStream } from "./hooks/useProcessStream";
import { useSystemMonitor } from "./hooks/useSystemMonitor";
import { alertText } from "./lib/alert";
import { presentError } from "./lib/error_ui";
import { formatBytes } from "./lib/format";
import { HISTORY_LIMIT } from "./lib/trend";
import { INTERVAL_OPTIONS, PREF, cardBgGradient, gradientText } from "./lib/ui";
import { AlertSettingsDrawer } from "./components/AlertSettingsDrawer";
import { CleanupConfirmModal } from "./components/CleanupConfirmModal";
import { ErrorBoundary } from "./components/ErrorBoundary";
import { KillConfirmModal } from "./components/KillConfirmModal";
import { MiniMenuBar } from "./components/MiniMenuBar";
import { PrefsTransferModal } from "./components/PrefsTransferModal";
import { ProcessDetailDrawer } from "./components/ProcessDetailDrawer";
import { AgentPanel } from "./agent/AgentPanel";
import { CleanupTab } from "./components/tabs/CleanupTab";
import { DiskTab } from "./components/tabs/DiskTab";
import { LargeFileTab } from "./components/tabs/LargeFileTab";
import { NetworkTab } from "./components/tabs/NetworkTab";
import { OverviewTab } from "./components/tabs/OverviewTab";
import { ProcessTab } from "./components/tabs/ProcessTab";
import { StartupTab } from "./components/tabs/StartupTab";
import { SystemInfoTab } from "./components/tabs/SystemInfoTab";

const { Header, Content } = Layout;
const { Title, Text } = Typography;

function App() {
  const {
    darkMode,
    setDarkMode,
    intervalMs,
    setIntervalMs,
    trendRange,
    setTrendRange,
    activeTab,
    setActiveTab,
    alertConfig,
    setAlertConfig,
    prefsSnapshot,
    applyPrefs,
  } = usePrefs();
  const [msgApi, msgContext] = message.useMessage();
  // 迷你模式（T5-07）：刻意只做会话级开关，不进 `usePrefs` 的那 5 项快照 ——
  // 加第 6 项要动 `PrefsSnapshot` 的字段与 T5-11 的 Rust/TS 契约测试，而偏好文件的
  // `schemaVersion` 仍是 1、且明确不做猜测式迁移，老文件里没有这一项反而是对的。
  const [miniMode, setMiniMode] = useState(false);

  // 失败一律经 presentError：按后端 code 决定级别，"进程已退出"这类不该报成红色故障。
  const reportError = useCallback(
    (context: string, e: unknown, onStale?: () => void) => {
      const notice = presentError(e, context);
      msgApi[notice.level](notice.message);
      if (notice.staleData) onStale?.();
    },
    [msgApi]
  );

  const { staticInfo, snapshot, history, status, seedInfo } = useSystemMonitor(
    HISTORY_LIMIT,
    intervalMs
  );
  const {
    query: processQuery,
    keyword: processKeyword,
    sort: processSort,
    page: processPage,
    pageSize: processPageSize,
    changeKeyword: changeProcessKeyword,
    changeSort: changeProcessSort,
    changePage: changeProcessPage,
    changePageSize: changeProcessPageSize,
    correctPageToRange: correctProcessPage,
  } = useProcessQuery();
  const processes = useProcessStream(activeTab === "processes" && !miniMode, processQuery);
  // 结果集变小或页容量改大时，旧偏移会落到末尾之后 —— 后端只会给空的一页，界面得自己回到范围内
  useEffect(() => correctProcessPage(processes.page.total), [
    correctProcessPage,
    processes.page.total,
  ]);

  // 阈值只有一条链：面板改动 → usePrefs 落盘 → useAlerts 下发 → 后端回夹取后的生效值覆盖输入框
  const alertNotice = useCallback(
    (event: AlertEvent) => {
      const text = alertText(event);
      if (event.level === "critical") {
        msgApi.error(text, 6);
      } else {
        msgApi.warning(text, 6);
      }
    },
    [msgApi]
  );
  const alerts = useAlerts(alertConfig, alertNotice, setAlertConfig);

  // 导出的是"图上正在看的那个范围"：范围由趋势图的 Segmented 决定，导出与显示同源才不会骗人
  const onExportCsv = useCallback(async () => {
    setCsvExporting(true);
    try {
      // 保存框本身也可能失败（对话框不可用、被系统拒），所以它必须在 try 里：
      // 放在外面会变成一次未捕获的 promise rejection，用户点了按钮什么也不会发生。
      const target = await saveDialog({
        defaultPath: `z-biz-tool-sys-history-${trendRange}s.csv`,
        filters: [{ name: "CSV", extensions: ["csv"] }],
      });
      if (!target) return; // 用户取消：不是错误，也不谎报"已导出"
      const outcome = await invoke<CsvExportOutcome>(Commands.exportHistoryCsv, {
        path: target,
        spanSeconds: trendRange,
      });
      const resolution =
        outcome.bucketSeconds === 10
          ? "10 s 原始采样点"
          : `每行是 ${outcome.bucketSeconds} s 的均值`;
      const broken = outcome.unreadableLines ? `，另有 ${outcome.unreadableLines} 行读不出未写入` : "";
      msgApi.success(`已导出 ${outcome.rows} 行（${resolution}，共 ${outcome.bytes} B${broken}）`);
    } catch (e) {
      reportError("导出历史 CSV", e);
    } finally {
      setCsvExporting(false);
    }
  }, [trendRange, msgApi, reportError]);
  const [alertPanelOpen, setAlertPanelOpen] = useState(false);
  // 偏好导入/导出（T5-11）：入口在"系统信息"页，弹层渲染在树尾，故页签白名单取自 tabItems
  const [prefsPanelOpen, setPrefsPanelOpen] = useState(false);
  // 诊断 Agent（T5-05）：面板本身零 IPC，问答只经这一条 `agent_query`；
  // 一次问答一份回复，不轮询 —— 面板是"问一句答一句"，不是又一件事件流。
  const [agentReply, setAgentReply] = useState<AgentReply | null>(null);
  const [askingAgent, setAskingAgent] = useState(false);

  const [junkReport, setJunkReport] = useState<JunkReport | null>(null);
  const [selectedJunkIds, setSelectedJunkIds] = useState<string[]>([]);
  const [scanning, setScanning] = useState(false);
  const [scanProgress, setScanProgress] = useState<ScanProgress | null>(null);
  const [cancelRequested, setCancelRequested] = useState(false);
  const [cleaning, setCleaning] = useState(false);
  const [cleanupConfirmOpen, setCleanupConfirmOpen] = useState(false);

  const [largeFiles, setLargeFiles] = useState<LargeFile[]>([]);
  const [largeFileMinSize, setLargeFileMinSize] = useState<number>(100);
  const [scanningLarge, setScanningLarge] = useState(false);

  const [startupItems, setStartupItems] = useState<StartupItem[]>([]);
  const [loadingStartup, setLoadingStartup] = useState(false);
  // 一次只允许一个启动项操作：两个 rename 抢同一个落点时，后一个会被「目标已被占」挡下，
  // 但那句拒绝是给机器看的，不该让用户在同一页里连点两下之后去猜哪次生效了
  const [startupBusy, setStartupBusy] = useState(false);
  // 温度/风扇（T5-08）：不进采集流，只在进系统信息页时读一次 + 手动刷新。
  // 读数本身可能永远为空（macOS 没有免提权通路），所以 null 与"空报告"是两种状态：
  // null = 还没查到（没试过，或那一次失败了），空报告 = 查到了、后端给了"为什么没有读数"的原因。
  // 历史趋势导出 CSV（02 的 F5）
  const [csvExporting, setCsvExporting] = useState(false);
  const [thermal, setThermal] = useState<ThermalReport | null>(null);
  const [loadingThermal, setLoadingThermal] = useState(false);
  // "这一页查过没有"要单独记一笔：失败时 thermal 会退回 null，若拿 `thermal === null` 当"没查过"，
  // 那次失败本身就把依赖改了、立刻再发一条 IPC（浏览器实测：点一次"重新读取"失败 → 2 条 get_thermal）。
  const [thermalTried, setThermalTried] = useState(false);

  const [killTarget, setKillTarget] = useState<KillValidation | null>(null);
  const [killConfirmText, setKillConfirmText] = useState("");
  const [killing, setKilling] = useState(false);

  const [detailPid, setDetailPid] = useState<number | null>(null);
  const [detail, setDetail] = useState<ProcessDetail | null>(null);
  const [detailLoading, setDetailLoading] = useState(false);

  // 详情是点击时的一次性读取，不进 3s 事件流；切换 PID 时旧响应不得覆盖新状态。
  useEffect(() => {
    if (detailPid === null) {
      setDetail(null);
      return;
    }
    let stale = false;
    setDetailLoading(true);
    invoke<ProcessDetail>(Commands.getProcessDetail, { pid: detailPid })
      .then((found) => {
        if (!stale) setDetail(found);
      })
      .catch((e) => {
        if (stale) return;
        setDetail(null);
        // 进程已退出（PID_NOT_FOUND）时顺带关掉抽屉，别留一个读不到东西的空抽屉
        reportError("读取进程详情", e, () => setDetailPid(null));
      })
      .finally(() => {
        if (!stale) setDetailLoading(false);
      });
    return () => {
      stale = true;
    };
  }, [detailPid, reportError]);

  // 采集频率交给后端，避免前后端两套节奏。
  useEffect(() => {
    invoke(Commands.setMonitorConfig, {
      config: {
        intervalMs,
        processIntervalMs: 3000,
        diskIntervalMs: 10000,
        paused: false,
      },
    }).catch((e) => reportError("设置采集频率", e));
  }, [intervalMs, reportError]);

  const scanJunk = useCallback(async () => {
    setScanning(true);
    setCancelRequested(false);
    setScanProgress(null);
    // 先挂上监听再 invoke：否则首轮进度帧会在请求发出到 effect 生效之间丢掉。
    let unlisten: UnlistenFn | undefined;
    try {
      unlisten = await listen<ScanProgress>(MonitorEvent.cleanupScanProgress, ({ payload }) =>
        setScanProgress(payload)
      );
    } catch {
      // 事件通道不可用时只是没有进度条，扫描本身照常
    }
    try {
      const report = await invoke<JunkReport>(Commands.scanJunkFiles);
      setJunkReport(report);
      setSelectedJunkIds(
        report.categories.filter((c) => c.riskLevel === "safe").map((c) => c.id)
      );
      if (report.cancelled) {
        msgApi.warning(
          `扫描已取消：只统计扫完的 ${report.categories.length} 类 / ${report.totalFiles} 个文件，结果不完整`
        );
      } else {
        msgApi.success(
          `扫描完成：发现 ${formatBytes(report.totalSizeBytes)} / ${report.totalFiles} 个文件`
        );
      }
    } catch (e) {
      reportError("扫描垃圾文件", e);
    } finally {
      unlisten?.();
      setScanning(false);
      setScanProgress(null);
      setCancelRequested(false);
    }
  }, [msgApi, reportError]);

  // 取消只置一个标志位，由扫描线程在下一个检查点自己停下（T3-09）。
  const cancelJunkScan = useCallback(async () => {
    try {
      const accepted = await invoke<boolean>(Commands.cancelJunkScan);
      if (accepted) {
        setCancelRequested(true);
        msgApi.info("已请求取消，扫描会在下一个检查点停下");
      } else {
        msgApi.warning("当前没有进行中的扫描");
      }
    } catch (e) {
      reportError("取消扫描", e);
    }
  }, [msgApi, reportError]);

  const doCleanup = useCallback(async () => {
    if (selectedJunkIds.length === 0) {
      msgApi.warning("请至少选择一个清理项");
      return;
    }
    setCleaning(true);
    setCleanupConfirmOpen(false);
    try {
      const result = await invoke<CleanupResult>(Commands.cleanupJunkFiles, {
        ids: selectedJunkIds,
      });
      const skipped =
        result.skippedPaths > 0 ? `，跳过 ${result.skippedPaths} 个受保护目录` : "";
      msgApi.success(
        `清理完成：释放 ${formatBytes(result.freedBytes)}，删除 ${result.deletedFiles} 个文件${skipped}`
      );
      if (result.failedFiles > 0) {
        msgApi.warning(`${result.failedFiles} 个文件未能删除：${result.errors[0] ?? "详见日志"}`);
      }
      await scanJunk();
    } catch (e) {
      reportError("清理垃圾文件", e);
    } finally {
      setCleaning(false);
    }
  }, [msgApi, reportError, scanJunk, selectedJunkIds]);

  const scanLargeFiles = useCallback(async () => {
    setScanningLarge(true);
    try {
      const files = await invoke<LargeFile[]>(Commands.findLargeFiles, {
        path: "~",
        minSizeMb: largeFileMinSize,
        limit: 50,
      });
      setLargeFiles(files);
      if (files.length === 0) {
        msgApi.info(`主目录之下没有大于 ${largeFileMinSize} MB 的文件`);
      } else {
        msgApi.success(`找到 ${files.length} 个大文件`);
      }
    } catch (e) {
      setLargeFiles([]);
      reportError("扫描大文件", e);
    } finally {
      setScanningLarge(false);
    }
  }, [largeFileMinSize, msgApi, reportError]);

  const loadStartupItems = useCallback(async () => {
    setLoadingStartup(true);
    try {
      setStartupItems(await invoke<StartupItem[]>(Commands.getStartupItems));
    } catch (e) {
      setStartupItems([]);
      reportError("加载启动项", e);
    } finally {
      setLoadingStartup(false);
    }
  }, [msgApi, reportError]);

  // 启动项三种动作（T3-08）：确认框里用户已经逐字核过名称，这里仍把同一个名称回给后端当第二道闸。
  const runStartupOp = useCallback(
    async (action: StartupAction, item: StartupItem) => {
      const command =
        action === "disable"
          ? Commands.disableStartupItem
          : action === "remove"
            ? Commands.removeStartupItem
            : Commands.restoreStartupItem;
      const label = action === "disable" ? "禁用" : action === "remove" ? "删除" : "恢复";
      setStartupBusy(true);
      try {
        const outcome = await invoke<StartupOutcome>(command, {
          id: item.id,
          confirmName: item.name,
        });
        // 结论按后端回的那一次动作说，而不是按我们「以为点了哪个按钮」
        msgApi.success(`${label}成功：${outcome.message}`);
      } catch (e) {
        reportError(`${label}启动项`, e);
      } finally {
        // 成功与否都要重扫：文件可能已被别的进程动过，界面上得是当前状态
        await loadStartupItems();
        setStartupBusy(false);
      }
    },
    [loadStartupItems, msgApi, reportError]
  );

  const onDisableStartup = useCallback(
    (item: StartupItem) => void runStartupOp("disable", item),
    [runStartupOp]
  );
  const onRemoveStartup = useCallback(
    (item: StartupItem) => void runStartupOp("remove", item),
    [runStartupOp]
  );
  const onRestoreStartup = useCallback(
    (item: StartupItem) => void runStartupOp("restore", item),
    [runStartupOp]
  );

  // 温度/风扇（T5-08）：不接收任何参数，能读的位置在后端是写死的常量。
  const loadThermal = useCallback(async () => {
    setLoadingThermal(true);
    try {
      setThermal(await invoke<ThermalReport>(Commands.getThermal));
    } catch (e) {
      // 失败时退回 null（而不是"空报告"），界面才会显示"正在读取"之外的第三种状态；
      // 具体错由 reportError 按 AppError.code 定级。
      setThermal(null);
      reportError("读取传感器", e);
    } finally {
      setLoadingThermal(false);
    }
  }, [msgApi, reportError]);

  // 温度不是每帧都变的东西，所以不进采集流：首次进系统信息页读一次，之后只由「重新读取」按钮再读
  useEffect(() => {
    if (activeTab === "system" && !thermalTried) {
      setThermalTried(true);
      void loadThermal();
    }
  }, [activeTab, thermalTried, loadThermal]);

  const flushDns = useCallback(async () => {    try {
      const result = await invoke<DnsFlushResult>(Commands.flushDns);
      if (result.flushed) {
        msgApi.success(result.message);
      } else {
        msgApi.warning(
          `${result.message}${result.manualCommand ? `；可在终端执行：${result.manualCommand}` : ""}`
        );
      }
    } catch (e) {
      reportError("刷新 DNS 缓存", e);
    }
  }, [msgApi, reportError]);

  // 结束进程：先向后端要真实进程名与风险级别，再决定确认强度。
  const requestKill = useCallback(
    async (pid: number) => {
      try {
        const validation = await invoke<KillValidation>(Commands.validateKill, { pid });
        if (!validation.allowed) {
          msgApi.error(validation.deniedReason ?? `PID ${pid} 不允许结束`);
          return;
        }
        setKillConfirmText("");
        setKillTarget(validation);
      } catch (e) {
        reportError("校验结束请求", e, () => processes.refresh());
      }
    },
    [msgApi, processes, reportError]
  );

  const confirmKill = useCallback(async () => {
    if (!killTarget) return;
    setKilling(true);
    try {
      const outcome = await invoke<{ pid: number; terminatedGracefully: boolean }>(
        Commands.killProcess,
        { pid: killTarget.pid, graceMs: 3000 }
      );
      msgApi.success(
        outcome.terminatedGracefully
          ? `${killTarget.processName} 已退出（SIGTERM）`
          : `${killTarget.processName} 未响应 SIGTERM，已升级为 SIGKILL`
      );
      setKillTarget(null);
      processes.refresh();
    } catch (e) {
      reportError("结束进程", e, () => processes.refresh());
    } finally {
      setKilling(false);
    }
  }, [killTarget, msgApi, processes, reportError]);

  // 诊断 Agent（T5-04）：唯一的 IPC。后端返回的是"结论 + 待确认的导航建议"，
  // 这里拿到什么就显示什么，不额外触发任何动作。
  const askAgent = useCallback(
    async (query: string) => {
      setAskingAgent(true);
      try {
        const reply = await invoke<AgentReply>(Commands.agentQuery, { query });
        // 后端承诺 executed 恒为 false；真出现 true 说明有一条执行通路漏了出来，直接拒显示。
        if (reply.executed) {
          setAgentReply(null);
          msgApi.error("Agent 返回了\"已执行\"的状态，已拒绝显示这条结果");
          return;
        }
        setAgentReply(reply);
      } catch (e) {
        reportError("诊断问答", e);
      } finally {
        setAskingAgent(false);
      }
    },
    [msgApi, reportError]
  );
  // 建议卡片只有这两个落点：切页签、把 PID 填进进程表关键字（后端按 pid 精确匹配）。
  const focusAgentProcess = useCallback(
    (pid: number) => {
      changeProcessKeyword(String(pid));
      setActiveTab("processes");
    },
    [changeProcessKeyword, setActiveTab]
  );

  // 迷你模式的键盘入口：`M` 切进/切出，`Esc` 切出。三条守卫都是必要的 ——
  // 进程表搜索框里打 m 不该换界面；弹层开着时 Esc 该归弹层自己关；带修饰键的组合留给系统。
  const overlayOpen =
    alertPanelOpen || prefsPanelOpen || cleanupConfirmOpen || detailPid !== null || killTarget !== null;
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.metaKey || event.ctrlKey || event.altKey || overlayOpen) return;
      const node = event.target as HTMLElement | null;
      const typing =
        !!node &&
        (node.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(node.tagName));
      if (typing) return;
      if (event.key === "Escape") setMiniMode(false);
      else if (event.key.toLowerCase() === "m") setMiniMode((prev) => !prev);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [overlayOpen]);

  const tabItems = [
    {
      key: "overview",
      label: "概览",
      children: (
        <ErrorBoundary label="概览页">
          <OverviewTab
            staticInfo={staticInfo}
            snapshot={snapshot}
            history={history}
            trendRange={trendRange}
            onTrendRangeChange={setTrendRange}
            seedInfo={seedInfo}
            alertEvents={alerts.events}
            onExportCsv={onExportCsv}
            csvExporting={csvExporting}
          />
        </ErrorBoundary>
      ),
    },
    {
      key: "agent",
      label: (
        <span>
          <RobotOutlined /> 诊断助手
        </span>
      ),
      children: (
        <ErrorBoundary label="诊断助手页">
          <AgentPanel
            reply={agentReply}
            asking={askingAgent}
            onAsk={askAgent}
            onOpenTab={setActiveTab}
            onFocusProcess={focusAgentProcess}
          />
        </ErrorBoundary>
      ),
    },
    {
      key: "processes",
      label: `进程${processes.page.total ? ` (${processes.page.total})` : ""}`,
      children: (
        <ErrorBoundary label="进程页">
          <ProcessTab
            page={processes.page}
            streaming={processes.streaming}
            hasSnapshot={snapshot !== null}
            keyword={processKeyword}
            onKeywordChange={changeProcessKeyword}
            sort={processSort}
            onSortChange={changeProcessSort}
            pageNumber={processPage}
            offset={processQuery.offset}
            pageSize={processPageSize}
            onPageSizeChange={changeProcessPageSize}
            onPageChange={changeProcessPage}
            onRefresh={processes.refresh}
            onOpenDetail={setDetailPid}
            onRequestKill={requestKill}
          />
        </ErrorBoundary>
      ),
    },
    {
      key: "network",
      label: "网络",
      children: (
        <ErrorBoundary label="网络页">
          <NetworkTab snapshot={snapshot} />
        </ErrorBoundary>
      ),
    },
    {
      key: "disk",
      label: "磁盘",
      children: (
        <ErrorBoundary label="磁盘页">
          <DiskTab snapshot={snapshot} />
        </ErrorBoundary>
      ),
    },
    {
      key: "cleanup",
      label: (
        <span>
          <ClearOutlined /> 系统清理
        </span>
      ),
      children: (
        <ErrorBoundary label="清理页">
          <CleanupTab
            report={junkReport}
            selectedIds={selectedJunkIds}
            onSelectedIdsChange={setSelectedJunkIds}
            scanning={scanning}
            cleaning={cleaning}
            progress={scanProgress}
            cancelRequested={cancelRequested}
            onScan={scanJunk}
            onCancelScan={cancelJunkScan}
            onRequestCleanup={() => setCleanupConfirmOpen(true)}
          />
        </ErrorBoundary>
      ),
    },
    {
      key: "largefiles",
      label: (
        <span>
          <FileSearchOutlined /> 大文件
        </span>
      ),
      children: (
        <ErrorBoundary label="大文件页">
          <LargeFileTab
            files={largeFiles}
            minSizeMb={largeFileMinSize}
            onMinSizeChange={setLargeFileMinSize}
            scanning={scanningLarge}
            onScan={scanLargeFiles}
          />
        </ErrorBoundary>
      ),
    },
    {
      key: "startup",
      label: (
        <span>
          <RocketOutlined /> 启动项
        </span>
      ),
      children: (
        <ErrorBoundary label="启动项页">
          <StartupTab
            items={startupItems}
            loading={loadingStartup}
            busy={startupBusy}
            onReload={loadStartupItems}
            onDisable={onDisableStartup}
            onRemove={onRemoveStartup}
            onRestore={onRestoreStartup}
          />
        </ErrorBoundary>
      ),
    },
    {
      key: "system",
      label: "系统信息",
      children: (
        <ErrorBoundary label="系统信息页">
          <SystemInfoTab
            staticInfo={staticInfo}
            snapshot={snapshot}
            thermal={thermal}
            thermalLoading={loadingThermal}
            onRefreshThermal={loadThermal}
            onTransferPrefs={() => setPrefsPanelOpen(true)}
          />
        </ErrorBoundary>
      ),
    },
  ];

  // 持久化的 tab key 可能是旧版本改名前的残留或手改值；antd 拿到不存在的 activeKey 时
  // 一个面板都不渲染（实测内容区直接空白）。以真实 items 为唯一事实来源回落，并把修正值写回。
  const activeKey = tabItems.some((item) => item.key === activeTab) ? activeTab : "overview";
  useEffect(() => {
    localStorage.setItem(PREF + "tab", activeKey);
  }, [activeKey]);

  return (
    <ConfigProvider
      theme={{ algorithm: darkMode ? theme.darkAlgorithm : theme.defaultAlgorithm }}
    >
      {msgContext}
      <Layout style={{ height: "100vh" }}>
        {!miniMode && (
        <Header
          style={{
            background: cardBgGradient,
            padding: "0 24px",
            borderBottom: "1px solid var(--ant-color-border-secondary)",
            display: "flex",
            justifyContent: "space-between",
            alignItems: "center",
          }}
        >
          <Space>
            <DashboardOutlined style={{ fontSize: 24, ...gradientText }} />
            <Title level={4} style={{ margin: 0, ...gradientText }}>
              系统监控仪表盘
            </Title>
          </Space>
          <Space size="middle">
            <Tooltip
              title={
                status === "live"
                  ? `后端采集正常，最近帧 ${snapshot ? dayjs(snapshot.timestampMs).format("HH:mm:ss") : "—"}`
                  : status === "stalled"
                    ? "超过 3 个周期未收到采集帧"
                    : "正在连接后端采集器"
              }
            >
              <Badge
                status={
                  status === "live" ? "success" : status === "stalled" ? "error" : "processing"
                }
                text={
                  status === "live" ? "实时采集" : status === "stalled" ? "采集停滞" : "连接中"
                }
              />
            </Tooltip>
            <Text type="secondary">
              {staticInfo
                ? `${staticInfo.hostname} | ${staticInfo.osName} ${staticInfo.osVersion}`
                : "读取系统信息…"}
            </Text>
            <Tooltip
              title={
                alertConfig.enabled
                  ? `连续 ${alertConfig.consecutive} 帧越限才报；本会话 ${alerts.recentCount} 条，含落盘历史共 ${alerts.events.length} 条`
                  : "告警处于静默：不再判定，也不会补报"
              }
            >
              <Badge count={alerts.events.length} size="small" offset={[-2, 2]}>
                <Button
                  icon={<BellOutlined />}
                  style={{ borderRadius: 6 }}
                  onClick={() => {
                    setAlertPanelOpen(true);
                    // 打开面板才回读落盘文件：告警是低频事件，没必要为此每秒起一次 IPC
                    alerts.refreshHistory();
                  }}
                >
                  告警阈值
                </Button>
              </Badge>
            </Tooltip>
            <Tooltip title="只显示关键指标（快捷键 M）。迷你模式不新增采集通路，也不动原生窗口尺寸。">
              <Button
                icon={<CompressOutlined />}
                style={{ borderRadius: 6 }}
                onClick={() => setMiniMode(true)}
              >
                迷你模式
              </Button>
            </Tooltip>
            <Select
              size="small"
              value={intervalMs}
              style={{ width: 96 }}
              options={INTERVAL_OPTIONS}
              onChange={setIntervalMs}
            />
            <Switch
              checked={darkMode}
              onChange={setDarkMode}
              checkedChildren="🌙"
              unCheckedChildren="☀️"
            />
            <Popconfirm
              title="刷新 DNS 缓存?"
              description="只会执行用户态命令，失败时会给出手动命令"
              onConfirm={flushDns}
            >
              <Button icon={<SyncOutlined />} style={{ borderRadius: 6 }}>
                刷新 DNS
              </Button>
            </Popconfirm>
          </Space>
        </Header>
        )}

        <Content style={{ padding: miniMode ? 0 : 16, overflow: miniMode ? "hidden" : "auto" }}>
          {miniMode ? (
            <ErrorBoundary label="迷你模式">
            <MiniMenuBar
              snapshot={snapshot}
              status={status}
              history={history}
              trendRange={trendRange}
              alertConfig={alertConfig}
              latestAlert={alerts.events[0] ?? null}
              intervalMs={intervalMs}
              onIntervalChange={setIntervalMs}
              onExit={() => setMiniMode(false)}
            />
            </ErrorBoundary>
          ) : (
            <>
          {status === "stalled" && (
            <Alert
              type="warning"
              showIcon
              style={{ marginBottom: 16 }}
              title="采集已停滞"
              description="后端在多个周期内没有推送新数据，当前数值停留在最后一帧；不会显示模拟数据。"
            />
          )}
          <Tabs
            activeKey={activeKey}
            onChange={setActiveTab}
            size="large"
            style={{ background: cardBgGradient, borderRadius: 16, overflow: "hidden" }}
            items={tabItems}
          />
            </>
          )}
        </Content>

        <CleanupConfirmModal
          open={cleanupConfirmOpen}
          report={junkReport}
          selectedIds={selectedJunkIds}
          cleaning={cleaning}
          onCancel={() => setCleanupConfirmOpen(false)}
          onConfirm={doCleanup}
        />

        <AlertSettingsDrawer
          open={alertPanelOpen}
          onClose={() => setAlertPanelOpen(false)}
          config={alertConfig}
          onChange={setAlertConfig}
          snapshot={snapshot}
          alerts={alerts}
        />

        <ProcessDetailDrawer
          pid={detailPid}
          detail={detail}
          loading={detailLoading}
          onClose={() => setDetailPid(null)}
          onOpenPid={setDetailPid}
        />

        <KillConfirmModal
          target={killTarget}
          confirmText={killConfirmText}
          killing={killing}
          onConfirmTextChange={setKillConfirmText}
          onCancel={() => setKillTarget(null)}
          onConfirm={confirmKill}
          afterClose={() => setKillConfirmText("")}
        />

        <PrefsTransferModal
          open={prefsPanelOpen}
          onClose={() => setPrefsPanelOpen(false)}
          /* 导出的是界面上真正生效的那个页签，不是 localStorage 里可能已过期的原值 */
          snapshot={{ ...prefsSnapshot, activeTab: activeKey }}
          tabs={tabItems.map((item) => item.key)}
          onApply={applyPrefs}
          onNotice={(level, text) => msgApi[level](text)}
        />
      </Layout>
    </ConfigProvider>
  );
}

export default App;
