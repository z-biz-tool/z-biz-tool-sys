import { useCallback, useEffect, useMemo, useState } from "react";
import {
  Alert,
  Badge,
  Button,
  Card,
  Col,
  ConfigProvider,
  Input,
  InputNumber,
  Layout,
  Modal,
  Popconfirm,
  Progress,
  Row,
  Select,
  Space,
  Statistic,
  Switch,
  Table,
  Tabs,
  Tag,
  Tooltip,
  Typography,
  message,
  theme,
} from "antd";
import {
  ClearOutlined,
  CloudOutlined,
  DashboardOutlined,
  DeleteOutlined,
  DesktopOutlined,
  FileSearchOutlined,
  HddOutlined,
  ReloadOutlined,
  RocketOutlined,
  SearchOutlined,
  StopOutlined,
  SyncOutlined,
  WifiOutlined,
} from "@ant-design/icons";
import {
  Area,
  AreaChart,
  CartesianGrid,
  ResponsiveContainer,
  Tooltip as ChartTooltip,
  XAxis,
  YAxis,
} from "recharts";
import dayjs from "dayjs";
import { invoke } from "@tauri-apps/api/core";
import {
  Commands,
  describeError,
  type CleanupResult,
  type DnsFlushResult,
  type JunkCategory,
  type JunkReport,
  type KillValidation,
  type LargeFile,
  type ProcessInfo,
  type StartupItem,
} from "./ipc_contract";
import { useProcessStream } from "./hooks/useProcessStream";
import { useSystemMonitor } from "./hooks/useSystemMonitor";
import {
  formatBytes,
  formatGb,
  formatPercent,
  formatRate,
  formatUptime,
  usageColor,
} from "./lib/format";

const brandGradient = "linear-gradient(135deg, #667eea 0%, #764ba2 100%)";
const cardBgGradient =
  "linear-gradient(135deg, rgba(102,126,234,0.04) 0%, rgba(118,75,162,0.04) 100%)";

const { Header, Content } = Layout;
const { Title, Text } = Typography;

const INTERVAL_OPTIONS = [
  { value: 500, label: "0.5 秒" },
  { value: 1000, label: "1 秒" },
  { value: 2000, label: "2 秒" },
  { value: 5000, label: "5 秒" },
];

const RISK_LABELS: Record<string, string> = {
  safe: "安全",
  moderate: "中等",
  risky: "高风险",
};

const gradientText = {
  background: brandGradient,
  WebkitBackgroundClip: "text",
  WebkitTextFillColor: "transparent",
} as const;

const monitorCardStyle = {
  borderRadius: 12,
  background: cardBgGradient,
  transition: "all 0.3s cubic-bezier(0.4, 0, 0.2, 1)",
};

function riskColor(level: string): string {
  if (level === "safe") return "green";
  if (level === "moderate") return "orange";
  if (level === "risky") return "red";
  return "default";
}

function MetricCard({
  icon,
  title,
  value,
  unit,
  percent,
  footer,
}: {
  icon: React.ReactNode;
  title: string;
  value: string;
  unit?: string;
  percent?: number;
  footer: React.ReactNode;
}) {
  return (
    <Card size="small" className="monitor-card" style={monitorCardStyle}>
      <div className="monitor-card-title" style={gradientText}>
        {icon} {title}
      </div>
      <div className="monitor-card-value">
        {value}
        {unit ? <span className="monitor-card-unit">{unit}</span> : null}
      </div>
      {percent === undefined ? (
        <div style={{ height: "8px" }} />
      ) : (
        <Progress
          percent={Math.min(percent, 100)}
          strokeColor={usageColor(percent, 80)}
          showInfo={false}
        />
      )}
      <Text type="secondary" style={{ fontSize: 12 }}>
        {footer}
      </Text>
    </Card>
  );
}

function TrendChart({
  title,
  data,
  dataKey,
  color,
  gradientId,
  yMax,
  unitFormatter,
}: {
  title: string;
  data: Array<Record<string, number | string>>;
  dataKey: string;
  color: string;
  gradientId: string;
  yMax?: number | "auto";
  unitFormatter: (v: number) => string;
}) {
  return (
    <Card
      size="small"
      className="monitor-card"
      style={{ ...monitorCardStyle }}
      title={<span style={{ ...gradientText, fontWeight: 600 }}>{title}</span>}
    >
      <ResponsiveContainer width="100%" height={200}>
        <AreaChart data={data}>
          <defs>
            <linearGradient id={gradientId} x1="0" y1="0" x2="0" y2="1">
              <stop offset="5%" stopColor={color} stopOpacity={0.8} />
              <stop offset="95%" stopColor={color} stopOpacity={0} />
            </linearGradient>
          </defs>
          <CartesianGrid strokeDasharray="3 3" />
          <XAxis dataKey="time" tick={{ fontSize: 12 }} minTickGap={24} />
          <YAxis domain={[0, yMax ?? "auto"]} tick={{ fontSize: 12 }} width={64} />
          <ChartTooltip formatter={(v) => unitFormatter(Number(v))} />
          <Area
            type="monotone"
            dataKey={dataKey}
            stroke={color}
            fill={`url(#${gradientId})`}
            isAnimationActive={false}
          />
        </AreaChart>
      </ResponsiveContainer>
    </Card>
  );
}

function App() {
  const [darkMode, setDarkMode] = useState(false);
  const [intervalMs, setIntervalMs] = useState(1000);
  const [activeTab, setActiveTab] = useState("overview");
  const [msgApi, msgContext] = message.useMessage();

  const { staticInfo, snapshot, history, status } = useSystemMonitor(120, intervalMs);
  const processes = useProcessStream(activeTab === "processes");

  const [junkReport, setJunkReport] = useState<JunkReport | null>(null);
  const [selectedJunkIds, setSelectedJunkIds] = useState<string[]>([]);
  const [scanning, setScanning] = useState(false);
  const [cleaning, setCleaning] = useState(false);

  const [largeFiles, setLargeFiles] = useState<LargeFile[]>([]);
  const [largeFileMinSize, setLargeFileMinSize] = useState<number>(100);
  const [scanningLarge, setScanningLarge] = useState(false);

  const [startupItems, setStartupItems] = useState<StartupItem[]>([]);
  const [loadingStartup, setLoadingStartup] = useState(false);

  const [processFilter, setProcessFilter] = useState("");
  const [killTarget, setKillTarget] = useState<KillValidation | null>(null);
  const [killConfirmText, setKillConfirmText] = useState("");
  const [killing, setKilling] = useState(false);

  // 采集频率交给后端，避免前后端两套节奏。
  useEffect(() => {
    invoke(Commands.setMonitorConfig, {
      config: {
        intervalMs,
        processIntervalMs: 3000,
        diskIntervalMs: 10000,
        paused: false,
      },
    }).catch((e) => msgApi.warning(`设置采集频率失败：${describeError(e)}`));
  }, [intervalMs, msgApi]);

  const cpu = snapshot?.cpu;
  const memory = snapshot?.memory;
  const networkTotal = useMemo(() => {
    if (!snapshot) return { rx: 0, tx: 0, ipv4: null as string | null };
    const active = snapshot.networks.filter((n) => n.status === "up" && n.ipv4);
    const pool = active.length ? active : snapshot.networks;
    return {
      rx: pool.reduce((a, n) => a + n.rxBytesPerSec, 0),
      tx: pool.reduce((a, n) => a + n.txBytesPerSec, 0),
      ipv4: pool.find((n) => n.ipv4)?.ipv4 ?? null,
    };
  }, [snapshot]);

  const diskTotals = useMemo(() => {
    const internal = snapshot?.disks.filter((d) => d.mountPoint === "/" || d.totalBytes > 0) ?? [];
    const total = internal.reduce((a, d) => a + d.totalBytes, 0);
    const used = internal.reduce((a, d) => a + d.usedBytes, 0);
    return { total, used, percent: total > 0 ? (used / total) * 100 : 0 };
  }, [snapshot]);

  const chartData = useMemo(
    () =>
      history.map((p) => ({
        time: dayjs(p.t).format("HH:mm:ss"),
        cpu: Number(p.cpu.toFixed(2)),
        memory: Number(p.memory.toFixed(2)),
        network: Number(((p.rxBytesPerSec + p.txBytesPerSec) / 1024).toFixed(2)),
        diskIo: 0,
      })),
    [history]
  );

  const visibleProcesses = useMemo(() => {
    const keyword = processFilter.trim().toLowerCase();
    const items = processes.page.items;
    if (!keyword) return items;
    return items.filter(
      (p) => p.name.toLowerCase().includes(keyword) || String(p.pid) === keyword
    );
  }, [processes.page.items, processFilter]);

  const scanJunk = useCallback(async () => {
    setScanning(true);
    try {
      const report = await invoke<JunkReport>(Commands.scanJunkFiles);
      setJunkReport(report);
      setSelectedJunkIds(
        report.categories.filter((c) => c.riskLevel === "safe").map((c) => c.id)
      );
      msgApi.success(
        `扫描完成：发现 ${formatBytes(report.totalSizeBytes)} / ${report.totalFiles} 个文件`
      );
    } catch (e) {
      msgApi.error(`扫描失败：${describeError(e)}`);
    } finally {
      setScanning(false);
    }
  }, [msgApi]);

  const doCleanup = useCallback(async () => {
    if (selectedJunkIds.length === 0) {
      msgApi.warning("请至少选择一个清理项");
      return;
    }
    setCleaning(true);
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
        msgApi.warning(
          `${result.failedFiles} 个文件未能删除：${result.errors[0] ?? "详见日志"}`
        );
      }
      await scanJunk();
    } catch (e) {
      msgApi.error(`清理失败：${describeError(e)}`);
    } finally {
      setCleaning(false);
    }
  }, [msgApi, scanJunk, selectedJunkIds]);

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
      msgApi.error(`扫描失败：${describeError(e)}`);
    } finally {
      setScanningLarge(false);
    }
  }, [largeFileMinSize, msgApi]);

  const loadStartupItems = useCallback(async () => {
    setLoadingStartup(true);
    try {
      setStartupItems(await invoke<StartupItem[]>(Commands.getStartupItems));
    } catch (e) {
      setStartupItems([]);
      msgApi.error(`加载失败：${describeError(e)}`);
    } finally {
      setLoadingStartup(false);
    }
  }, [msgApi]);

  const flushDns = useCallback(async () => {
    try {
      const result = await invoke<DnsFlushResult>(Commands.flushDns);
      if (result.flushed) {
        msgApi.success(result.message);
      } else {
        msgApi.warning(
          `${result.message}${result.manualCommand ? `；可在终端执行：${result.manualCommand}` : ""}`
        );
      }
    } catch (e) {
      msgApi.error(`刷新失败：${describeError(e)}`);
    }
  }, [msgApi]);

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
        msgApi.error(`校验失败：${describeError(e)}`);
      }
    },
    [msgApi]
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
      msgApi.error(`结束失败：${describeError(e)}`);
    } finally {
      setKilling(false);
    }
  }, [killTarget, msgApi, processes]);

  const processColumns = [
    { title: "PID", dataIndex: "pid", key: "pid", width: 90 },
    {
      title: "进程名",
      dataIndex: "name",
      key: "name",
      ellipsis: true,
      render: (name: string) => <Text strong>{name}</Text>,
    },
    {
      title: "CPU",
      dataIndex: "cpuUsage",
      key: "cpuUsage",
      width: 110,
      sorter: (a: ProcessInfo, b: ProcessInfo) => a.cpuUsage - b.cpuUsage,
      render: (value: number) =>
        processes.page.warming ? (
          <Text type="secondary">采样中…</Text>
        ) : (
          <span style={{ color: usageColor(value, 80) }}>{formatPercent(value)}</span>
        ),
    },
    {
      title: "内存",
      dataIndex: "memoryBytes",
      key: "memoryBytes",
      width: 130,
      sorter: (a: ProcessInfo, b: ProcessInfo) => a.memoryBytes - b.memoryBytes,
      render: (value: number) => formatBytes(value, 1),
    },
    {
      title: "运行时长",
      dataIndex: "runTimeSeconds",
      key: "runTimeSeconds",
      width: 140,
      render: (value: number | null) => formatUptime(value ?? 0),
    },
    {
      title: "操作",
      key: "action",
      width: 100,
      render: (_: unknown, record: ProcessInfo) => (
        <Button
          type="link"
          danger
          icon={<StopOutlined />}
          size="small"
          onClick={() => requestKill(record.pid)}
        >
          结束
        </Button>
      ),
    },
  ];

  return (
    <ConfigProvider theme={{ algorithm: darkMode ? theme.darkAlgorithm : theme.defaultAlgorithm }}>
      {msgContext}
      <Layout style={{ height: "100vh" }}>
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
                status={status === "live" ? "success" : status === "stalled" ? "error" : "processing"}
                text={status === "live" ? "实时采集" : status === "stalled" ? "采集停滞" : "连接中"}
              />
            </Tooltip>
            <Text type="secondary">
              {staticInfo
                ? `${staticInfo.hostname} | ${staticInfo.osName} ${staticInfo.osVersion}`
                : "读取系统信息…"}
            </Text>
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

        <Content style={{ padding: 16, overflow: "auto" }}>
          {status === "stalled" && (
            <Alert
              type="warning"
              showIcon
              style={{ marginBottom: 16 }}
              message="采集已停滞"
              description="后端在多个周期内没有推送新数据，当前数值停留在最后一帧；不会显示模拟数据。"
            />
          )}
          <Tabs activeKey={activeTab} onChange={setActiveTab} size="large" style={{ background: cardBgGradient, borderRadius: 16, overflow: "hidden" }}>
            {/* ============ 概览 ============ */}
            <Tabs.TabPane tab="概览" key="overview">
              <Row gutter={[16, 16]} style={{ marginBottom: 16 }}>
                <Col span={6}>
                  <MetricCard
                    icon={<DesktopOutlined />}
                    title="CPU 使用率"
                    value={cpu ? formatPercent(cpu.total) : "—"}
                    percent={cpu?.total}
                    footer={
                      staticInfo
                        ? `${staticInfo.cpuModel} (${staticInfo.coreCount} 核心)`
                        : "读取中…"
                    }
                  />
                </Col>
                <Col span={6}>
                  <MetricCard
                    icon={<CloudOutlined />}
                    title="内存使用"
                    value={memory ? formatGb(memory.usedBytes) : "—"}
                    unit="GB"
                    percent={memory?.usagePercent}
                    footer={
                      memory
                        ? `共 ${formatGb(memory.totalBytes)} · ${memory.pressure}`
                        : "等待首帧…"
                    }
                  />
                </Col>
                <Col span={6}>
                  <MetricCard
                    icon={<HddOutlined />}
                    title="磁盘使用"
                    value={snapshot ? formatGb(diskTotals.used) : "—"}
                    unit="GB"
                    percent={snapshot ? diskTotals.percent : undefined}
                    footer={snapshot ? `共 ${formatGb(diskTotals.total)}` : "等待首帧…"}
                  />
                </Col>
                <Col span={6}>
                  <MetricCard
                    icon={<WifiOutlined />}
                    title="网络流量"
                    value={
                      snapshot
                        ? `${formatBytes(networkTotal.rx, 0)}/s`
                        : "—"
                    }
                    footer={
                      snapshot
                        ? `↓ ${formatRate(networkTotal.rx)} · ↑ ${formatRate(networkTotal.tx)} · ${networkTotal.ipv4 ?? "无 IPv4"}`
                        : "等待首帧…"
                    }
                  />
                </Col>
              </Row>

              <Row gutter={[16, 16]}>
                <Col span={12}>
                  <TrendChart
                    title="CPU 使用率趋势"
                    data={chartData}
                    dataKey="cpu"
                    color="#667eea"
                    gradientId="colorCpu"
                    yMax={100}
                    unitFormatter={(v) => `${v.toFixed(1)}%`}
                  />
                </Col>
                <Col span={12}>
                  <TrendChart
                    title="内存使用趋势"
                    data={chartData}
                    dataKey="memory"
                    color="#764ba2"
                    gradientId="colorMemory"
                    yMax={100}
                    unitFormatter={(v) => `${v.toFixed(1)}%`}
                  />
                </Col>
              </Row>
            </Tabs.TabPane>

            {/* ============ 进程 ============ */}
            <Tabs.TabPane tab={`进程${processes.page.total ? ` (${processes.page.total})` : ""}`} key="processes">
              <Card
                title="进程列表"
                className="monitor-card"
                extra={
                  <Space>
                    <Input
                      allowClear
                      prefix={<SearchOutlined />}
                      placeholder="按名称或 PID 过滤"
                      value={processFilter}
                      onChange={(e) => setProcessFilter(e.target.value)}
                      style={{ width: 220 }}
                    />
                    <Button icon={<ReloadOutlined />} onClick={processes.refresh}>
                      刷新
                    </Button>
                  </Space>
                }
              >
                {!processes.streaming && chartData.length === 0 ? (
                  <Alert type="info" showIcon message="正在建立进程采集…" />
                ) : null}
                <Table<ProcessInfo>
                  rowKey="pid"
                  columns={processColumns}
                  dataSource={visibleProcesses}
                  size="small"
                  loading={visibleProcesses.length === 0 && processes.streaming}
                  pagination={{ pageSize: 20, showSizeChanger: true }}
                  locale={{ emptyText: "尚未收到进程数据" }}
                />
              </Card>
            </Tabs.TabPane>

            {/* ============ 网络 ============ */}
            <Tabs.TabPane tab="网络" key="network">
              <Card title="网络接口" className="monitor-card">
                <Table
                  rowKey="interface"
                  columns={[
                    { title: "接口", dataIndex: "interface", key: "interface", width: 120 },
                    {
                      title: "状态",
                      dataIndex: "status",
                      key: "status",
                      width: 90,
                      render: (s: string) => (
                        <Tag color={s === "up" ? "success" : s === "down" ? "error" : "default"}>
                          {s === "up" ? "在线" : s === "down" ? "离线" : "未知"}
                        </Tag>
                      ),
                    },
                    {
                      title: "IPv4",
                      dataIndex: "ipv4",
                      key: "ipv4",
                      render: (v: string | null) => v ?? "—",
                    },
                    {
                      title: "IPv6",
                      dataIndex: "ipv6",
                      key: "ipv6",
                      ellipsis: true,
                      render: (v: string | null) => v ?? "—",
                    },
                    {
                      title: "MAC",
                      dataIndex: "mac",
                      key: "mac",
                      width: 160,
                      render: (v: string | null) => (v ? <Text code>{v}</Text> : "—"),
                    },
                    {
                      title: "下行",
                      dataIndex: "rxBytesPerSec",
                      key: "rx",
                      width: 120,
                      render: (v: number) => formatRate(v),
                    },
                    {
                      title: "上行",
                      dataIndex: "txBytesPerSec",
                      key: "tx",
                      width: 120,
                      render: (v: number) => formatRate(v),
                    },
                  ]}
                  dataSource={snapshot?.networks ?? []}
                  size="small"
                  pagination={false}
                  locale={{ emptyText: "等待采集首帧…" }}
                />
              </Card>
            </Tabs.TabPane>

            {/* ============ 磁盘 ============ */}
            <Tabs.TabPane tab="磁盘" key="disk">
              <Card title="分区" className="monitor-card">
                <Table
                  rowKey="mountPoint"
                  columns={[
                    { title: "名称", dataIndex: "name", key: "name" },
                    {
                      title: "挂载点",
                      dataIndex: "mountPoint",
                      key: "mountPoint",
                      render: (v: string) => <Text code>{v}</Text>,
                    },
                    { title: "文件系统", dataIndex: "fileSystem", key: "fileSystem", width: 140 },
                    { title: "容量", dataIndex: "totalBytes", key: "total", width: 120, render: (v: number) => formatBytes(v, 0) },
                    { title: "已用", dataIndex: "usedBytes", key: "used", width: 120, render: (v: number) => formatBytes(v, 0) },
                    { title: "可用", dataIndex: "availableBytes", key: "free", width: 120, render: (v: number) => formatBytes(v, 0) },
                    {
                      title: "使用率",
                      dataIndex: "usagePercent",
                      key: "usagePercent",
                      width: 160,
                      render: (v: number) => (
                        <Progress
                          percent={Number(v.toFixed(1))}
                          size="small"
                          strokeColor={usageColor(v, 90)}
                        />
                      ),
                    },
                  ]}
                  dataSource={snapshot?.disks ?? []}
                  size="small"
                  pagination={false}
                  locale={{ emptyText: "等待采集首帧…" }}
                />
              </Card>
            </Tabs.TabPane>

            {/* ============ 系统清理 ============ */}
            <Tabs.TabPane
              tab={
                <span>
                  <ClearOutlined /> 系统清理
                </span>
              }
              key="cleanup"
            >
              <Card
                title={
                  <Space>
                    <ClearOutlined style={{ color: "#52c41a" }} />
                    <span>垃圾文件扫描与清理</span>
                  </Space>
                }
                extra={
                  <Space>
                    <Button type="primary" icon={<SearchOutlined />} onClick={scanJunk} loading={scanning}>
                      扫描垃圾
                    </Button>
                    <Popconfirm
                      title="确认清理选中项?"
                      description="文件删除后不可恢复"
                      onConfirm={doCleanup}
                      disabled={!junkReport || selectedJunkIds.length === 0}
                    >
                      <Button
                        danger
                        icon={<DeleteOutlined />}
                        loading={cleaning}
                        disabled={!junkReport || selectedJunkIds.length === 0}
                      >
                        清理选中 ({selectedJunkIds.length})
                      </Button>
                    </Popconfirm>
                  </Space>
                }
              >
                {!junkReport ? (
                  <div style={{ textAlign: "center", padding: "60px 0", color: "var(--ant-color-text-secondary)" }}>
                    <ClearOutlined style={{ fontSize: 64, marginBottom: 16 }} />
                    <div style={{ fontSize: 16, marginBottom: 8 }}>点击“扫描垃圾”开始分析</div>
                    <div style={{ fontSize: 12 }}>扫描缓存、临时文件、回收站等；受保护目录会被自动跳过</div>
                  </div>
                ) : (
                  <>
                    <Row gutter={16} style={{ marginBottom: 16 }}>
                      <Col span={8}>
                        <Statistic
                          title="可清理空间"
                          value={formatBytes(junkReport.totalSizeBytes)}
                          valueStyle={{ color: "#52c41a", fontSize: 28 }}
                        />
                      </Col>
                      <Col span={8}>
                        <Statistic title="可清理文件数" value={junkReport.totalFiles.toLocaleString()} prefix={<FileSearchOutlined />} />
                      </Col>
                      <Col span={8}>
                        <Statistic title="扫描耗时" value={`${junkReport.scanTimeMs} ms`} valueStyle={{ color: "#1890ff" }} />
                      </Col>
                    </Row>

                    <Table<JunkCategory>
                      rowKey="id"
                      rowSelection={{
                        selectedRowKeys: selectedJunkIds,
                        onChange: (keys) => setSelectedJunkIds(keys as string[]),
                      }}
                      columns={[
                        {
                          title: "类别",
                          dataIndex: "name",
                          render: (name: string, record) => (
                            <Space direction="vertical" size={0}>
                              <strong>{name}</strong>
                              <Text type="secondary" style={{ fontSize: 12 }}>
                                {record.description}
                              </Text>
                            </Space>
                          ),
                        },
                        {
                          title: "风险等级",
                          dataIndex: "riskLevel",
                          width: 100,
                          render: (level: string) => <Tag color={riskColor(level)}>{RISK_LABELS[level] ?? level}</Tag>,
                        },
                        { title: "文件数", dataIndex: "fileCount", width: 120, render: (n: number) => n.toLocaleString() },
                        {
                          title: "占用空间",
                          dataIndex: "sizeBytes",
                          width: 150,
                          render: (size: number) => (
                            <span style={{ fontWeight: 600, color: "#52c41a" }}>{formatBytes(size)}</span>
                          ),
                        },
                        {
                          title: "路径",
                          dataIndex: "paths",
                          render: (paths: string[]) => (
                            <Space direction="vertical" size={0}>
                              {paths.slice(0, 2).map((p, i) => (
                                <Text key={i} code style={{ fontSize: 11 }}>
                                  {p}
                                </Text>
                              ))}
                              {paths.length > 2 && (
                                <Text type="secondary" style={{ fontSize: 11 }}>
                                  …等 {paths.length} 个
                                </Text>
                              )}
                            </Space>
                          ),
                        },
                      ]}
                      dataSource={junkReport.categories}
                      pagination={false}
                      size="middle"
                    />
                  </>
                )}
              </Card>
            </Tabs.TabPane>

            {/* ============ 大文件 ============ */}
            <Tabs.TabPane
              tab={
                <span>
                  <FileSearchOutlined /> 大文件
                </span>
              }
              key="largefiles"
            >
              <Card
                title={
                  <Space>
                    <FileSearchOutlined style={{ color: "#faad14" }} />
                    <span>大文件扫描（用户主目录之下）</span>
                  </Space>
                }
                extra={
                  <Space>
                    <span>最小大小 (MB):</span>
                    <InputNumber min={1} value={largeFileMinSize} onChange={(v) => setLargeFileMinSize(v || 100)} style={{ width: 120 }} />
                    <Button type="primary" icon={<SearchOutlined />} onClick={scanLargeFiles} loading={scanningLarge}>
                      扫描
                    </Button>
                  </Space>
                }
              >
                {largeFiles.length === 0 ? (
                  <div style={{ textAlign: "center", padding: "60px 0", color: "var(--ant-color-text-secondary)" }}>
                    <FileSearchOutlined style={{ fontSize: 64, marginBottom: 16 }} />
                    <div>点击“扫描”开始查找大文件</div>
                  </div>
                ) : (
                  <Table<LargeFile>
                    rowKey="path"
                    columns={[
                      { title: "文件路径", dataIndex: "path", ellipsis: true, render: (p: string) => <Text code style={{ fontSize: 12 }}>{p}</Text> },
                      {
                        title: "大小",
                        dataIndex: "sizeBytes",
                        width: 150,
                        sorter: (a, b) => a.sizeBytes - b.sizeBytes,
                        defaultSortOrder: "descend" as const,
                        render: (s: number) => <span style={{ fontWeight: 600 }}>{formatBytes(s)}</span>,
                      },
                      {
                        title: "修改时间",
                        dataIndex: "modifiedSeconds",
                        width: 180,
                        render: (t: number) => (t > 0 ? dayjs.unix(t).format("YYYY-MM-DD HH:mm") : "—"),
                      },
                    ]}
                    dataSource={largeFiles}
                    pagination={{ pageSize: 20 }}
                  />
                )}
              </Card>
            </Tabs.TabPane>

            {/* ============ 启动项 ============ */}
            <Tabs.TabPane
              tab={
                <span>
                  <RocketOutlined /> 启动项
                </span>
              }
              key="startup"
            >
              <Card
                title={
                  <Space>
                    <RocketOutlined style={{ color: "#722ed1" }} />
                    <span>开机启动项（只读枚举）</span>
                  </Space>
                }
                extra={
                  <Button icon={<ReloadOutlined />} onClick={loadStartupItems} loading={loadingStartup}>
                    刷新
                  </Button>
                }
              >
                <Table<StartupItem>
                  rowKey="id"
                  columns={[
                    { title: "名称", dataIndex: "name", render: (n: string) => <strong>{n}</strong> },
                    { title: "来源", dataIndex: "source", width: 150, render: (s: string) => <Tag color="blue">{s}</Tag> },
                    { title: "命令/路径", dataIndex: "command", render: (c: string) => <Text code style={{ fontSize: 12 }}>{c}</Text> },
                    {
                      title: "状态",
                      dataIndex: "enabled",
                      width: 100,
                      render: (e: boolean) => (e ? <Tag color="success">已启用</Tag> : <Tag>已禁用</Tag>),
                    },
                    {
                      title: "操作",
                      key: "action",
                      width: 140,
                      render: () => (
                        <Tooltip title="禁用/删除启动项尚未实现（Phase 3），不会执行任何写操作">
                          <Space size="small">
                            <Button size="small" disabled>
                              禁用
                            </Button>
                            <Button size="small" danger disabled>
                              删除
                            </Button>
                          </Space>
                        </Tooltip>
                      ),
                    },
                  ]}
                  dataSource={startupItems}
                  loading={loadingStartup}
                  pagination={false}
                  locale={{ emptyText: "点击“刷新”读取系统启动项" }}
                />
              </Card>
            </Tabs.TabPane>

            {/* ============ 系统信息 ============ */}
            <Tabs.TabPane tab="系统信息" key="system">
              <Card title="系统信息" className="monitor-card">
                {staticInfo ? (
                  <Row gutter={[16, 16]}>
                    <Col span={12}>
                      <Statistic title="主机名" value={staticInfo.hostname} />
                    </Col>
                    <Col span={12}>
                      <Statistic title="操作系统" value={`${staticInfo.osName} ${staticInfo.osVersion}`} />
                    </Col>
                    <Col span={12}>
                      <Statistic title="内核版本" value={staticInfo.kernelVersion} />
                    </Col>
                    <Col span={12}>
                      <Statistic title="运行时间" value={snapshot ? formatUptime(snapshot.uptimeSeconds) : "—"} />
                    </Col>
                    <Col span={12}>
                      <Statistic title="CPU 型号" value={staticInfo.cpuModel} />
                    </Col>
                    <Col span={12}>
                      <Statistic title="CPU 核心数" value={staticInfo.coreCount} />
                    </Col>
                    <Col span={12}>
                      <Statistic title="总内存" value={formatGb(staticInfo.totalMemoryBytes)} />
                    </Col>
                    <Col span={12}>
                      <Statistic
                        title="已用内存"
                        value={memory ? formatGb(memory.usedBytes) : "—"}
                        valueStyle={{ color: memory ? usageColor(memory.usagePercent, 80) : undefined }}
                      />
                    </Col>
                    <Col span={12}>
                      <Statistic title="平台 / 架构" value={`${staticInfo.platform} / ${staticInfo.arch}`} />
                    </Col>
                    <Col span={12}>
                      <Statistic title="当前用户" value={staticInfo.currentUserName ?? "—"} />
                    </Col>
                  </Row>
                ) : (
                  <Alert type="info" showIcon message="正在读取系统信息…" />
                )}
              </Card>
            </Tabs.TabPane>
          </Tabs>
        </Content>

        <Modal
          open={!!killTarget}
          title={`结束进程 ${killTarget?.processName ?? ""}`}
          onCancel={() => setKillTarget(null)}
          onOk={confirmKill}
          okText="确认结束"
          okButtonProps={{
            danger: true,
            loading: killing,
            disabled: killTarget?.riskLevel === "critical" && killConfirmText !== killTarget?.processName,
          }}
          afterClose={() => setKillConfirmText("")}
        >
          {killTarget ? (
            <Space direction="vertical" size="small" style={{ width: "100%" }}>
              <Text>
                PID <Text code>{killTarget.pid}</Text> · 内存 {formatBytes(killTarget.memoryBytes, 1)}
                {killTarget.userName ? ` · 属主 ${killTarget.userName}` : ""}
              </Text>
              <Text type="danger">
                {killTarget.riskLevel === "critical"
                  ? "该进程中断会立刻影响图形会话，请输入进程名以确认。"
                  : "结束该进程可能丢失未保存的数据。"}
              </Text>
              {killTarget.riskLevel === "critical" && (
                <Input
                  placeholder={`输入 ${killTarget.processName} 以确认`}
                  value={killConfirmText}
                  onChange={(e) => setKillConfirmText(e.target.value)}
                />
              )}
            </Space>
          ) : null}
        </Modal>
      </Layout>
    </ConfigProvider>
  );
}

export default App;
