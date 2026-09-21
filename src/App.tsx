import { useState, useEffect } from "react";
import { ConfigProvider, theme, Layout, Card, Row, Col, Statistic, Switch, Space, Typography, Tabs, Table, Progress, Button, message, Tag, Popconfirm, Tooltip, InputNumber } from "antd";
import {
  DashboardOutlined,
  DesktopOutlined,
  CloudOutlined,
  HddOutlined,
  WifiOutlined,
  ReloadOutlined,
  SettingOutlined,
  DeleteOutlined,
  SearchOutlined,
  RocketOutlined,
  ClearOutlined,
  StopOutlined,
  SyncOutlined,
  FileSearchOutlined,
} from "@ant-design/icons";
import { XAxis, YAxis, CartesianGrid, Tooltip as ChartTooltip, ResponsiveContainer, AreaChart, Area } from "recharts";
import { invoke } from "@tauri-apps/api/core";
import dayjs from "dayjs";

// 渐变色主题常量
const brandGradient = "linear-gradient(135deg, #667eea 0%, #764ba2 100%)";
const cardBgGradient = "linear-gradient(135deg, rgba(102,126,234,0.04) 0%, rgba(118,75,162,0.04) 100%)";

const { Header, Content } = Layout;
const { Title, Text } = Typography;

// 类型定义
interface JunkCategory {
  id: string;
  name: string;
  description: string;
  paths: string[];
  size: number;
  file_count: number;
  risk_level: string;
}

interface JunkReport {
  total_size: number;
  total_files: number;
  categories: JunkCategory[];
  scan_time_ms: number;
}

interface CleanupResult {
  freed_bytes: number;
  deleted_files: number;
  failed_files: number;
  errors: string[];
}

interface LargeFile {
  path: string;
  size: number;
  modified: number;
  is_dir: boolean;
}

interface StartupItem {
  id: string;
  name: string;
  command: string;
  source: string;
  enabled: boolean;
  location: string;
}

// 工具函数：格式化字节
const formatBytes = (bytes: number): string => {
  if (bytes === 0) return "0 B";
  const k = 1024;
  const sizes = ["B", "KB", "MB", "GB", "TB"];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return `${(bytes / Math.pow(k, i)).toFixed(2)} ${sizes[i]}`;
};

// 模拟数据生成（CPU/内存趋势图）
const generateMockData = (count: number) => {
  const data = [];
  const now = dayjs();
  for (let i = count - 1; i >= 0; i--) {
    data.push({
      time: now.subtract(i, "second").format("HH:mm:ss"),
      cpu: Math.random() * 100,
      memory: 40 + Math.random() * 30,
      disk: 60 + Math.random() * 10,
      network: Math.random() * 1000,
    });
  }
  return data;
};

// 模拟系统信息
const mockSystemInfo = {
  hostname: "zifang-macbook",
  os: "macOS 15.0.1",
  kernel: "Darwin 24.0.0",
  uptime: "3天 12小时 45分钟",
  cpuModel: "Apple M3 Pro",
  cpuCores: 12,
  totalMemory: 18.0,
  usedMemory: 12.5,
  diskTotal: 512.0,
  diskUsed: 280.0,
  networkInterfaces: [
    { name: "en0", ip: "192.168.1.100", mac: "AA:BB:CC:DD:EE:FF", speed: 1000 },
    { name: "en1", ip: "10.0.0.1", mac: "11:22:33:44:55:66", speed: 1000 },
  ],
};

function App() {
  const [darkMode, setDarkMode] = useState(false);
  const [systemInfo] = useState(mockSystemInfo);
  const [monitorData, setMonitorData] = useState(generateMockData(60));
const [refreshInterval] = useState(1);
  const [activeTab, setActiveTab] = useState("overview");
  const [msgApi, msgContext] = message.useMessage();

  // 系统清理状态
  const [junkReport, setJunkReport] = useState<JunkReport | null>(null);
  const [selectedJunkIds, setSelectedJunkIds] = useState<string[]>([]);
  const [scanning, setScanning] = useState(false);
  const [cleaning, setCleaning] = useState(false);

  // 大文件状态
  const [largeFiles, setLargeFiles] = useState<LargeFile[]>([]);
  const [largeFileMinSize, setLargeFileMinSize] = useState<number>(100);
  const [scanningLarge, setScanningLarge] = useState(false);
  const [refreshInterval] = useState(1);

  // 启动项状态
  const [startupItems, setStartupItems] = useState<StartupItem[]>([]);
  const [loadingStartup, setLoadingStartup] = useState(false);

  // 模拟实时数据更新
  useEffect(() => {
    const interval = setInterval(() => {
      setMonitorData((prev) => {
        const newData = [...prev.slice(1)];
        const now = dayjs();
        newData.push({
          time: now.format("HH:mm:ss"),
          cpu: Math.random() * 100,
          memory: 40 + Math.random() * 30,
          disk: 60 + Math.random() * 10,
          network: Math.random() * 1000,
        });
        return newData;
      });
    }, refreshInterval * 1000);

    return () => clearInterval(interval);
  }, [refreshInterval]);

  // 获取状态颜色
  const getStatusColor = (value: number, threshold: number) => {
    if (value >= threshold) return "#ff4d4f";
    if (value >= threshold * 0.8) return "#faad14";
    return "#52c41a";
  };

  // 风险等级颜色
  const getRiskColor = (level: string) => {
    switch (level) {
      case "safe":
        return "green";
      case "moderate":
        return "orange";
      case "risky":
        return "red";
      default:
        return "default";
    }
  };

  // 扫描垃圾文件
  const scanJunk = async () => {
    setScanning(true);
    try {
      const report = await invoke<JunkReport>("scan_junk_files");
      setJunkReport(report);
      // 默认选中 safe 类别
      setSelectedJunkIds(
        report.categories.filter((c) => c.risk_level === "safe").map((c) => c.id)
      );
      msgApi.success(`扫描完成：发现 ${formatBytes(report.total_size)} 垃圾文件`);
    } catch (e: any) {
      msgApi.warning(`扫描失败：${e}，显示模拟数据`);
      // 提供模拟数据
      const mockReport: JunkReport = {
        total_size: 2.4 * 1024 * 1024 * 1024,
        total_files: 8421,
        scan_time_ms: 1234,
        categories: [
          { id: "user_cache", name: "用户缓存", description: "用户级缓存目录", paths: ["~/.cache"], size: 850 * 1024 * 1024, file_count: 3241, risk_level: "safe" },
          { id: "browser_cache", name: "浏览器缓存", description: "Chrome/Brave 缓存", paths: ["~/.cache/google-chrome"], size: 620 * 1024 * 1024, file_count: 1820, risk_level: "moderate" },
          { id: "npm_cache", name: "NPM 缓存", description: "NPM 包缓存", paths: ["~/.npm"], size: 480 * 1024 * 1024, file_count: 2103, risk_level: "moderate" },
          { id: "macos_xcode", name: "Xcode 派生数据", description: "Xcode 编译缓存", paths: ["~/Library/Developer/Xcode/DerivedData"], size: 320 * 1024 * 1024, file_count: 845, risk_level: "moderate" },
          { id: "trash", name: "回收站", description: "已删除文件", paths: ["~/.local/share/Trash"], size: 156 * 1024 * 1024, file_count: 412, risk_level: "moderate" },
        ],
      };
      setJunkReport(mockReport);
      setSelectedJunkIds(mockReport.categories.filter((c) => c.risk_level === "safe").map((c) => c.id));
    } finally {
      setScanning(false);
    }
  };

  // 执行清理
  const doCleanup = async () => {
    if (selectedJunkIds.length === 0) {
      msgApi.warning("请至少选择一个清理项");
      return;
    }
    setCleaning(true);
    try {
      const result = await invoke<CleanupResult>("cleanup_junk_files", { ids: selectedJunkIds });
      msgApi.success(`清理完成：释放 ${formatBytes(result.freed_bytes)}，删除 ${result.deleted_files} 个文件`);
      await scanJunk(); // 重新扫描
    } catch (e: any) {
      msgApi.warning(`清理失败：${e}`);
    } finally {
      setCleaning(false);
    }
  };

  // 扫描大文件
  const scanLargeFiles = async () => {
    setScanningLarge(true);
    try {
      const home = "~";
      const files = await invoke<LargeFile[]>("find_large_files_cmd", {
        path: home,
        minSizeMb: largeFileMinSize,
        limit: 50,
      });
      setLargeFiles(files);
      msgApi.success(`找到 ${files.length} 个大文件`);
    } catch (e: any) {
      msgApi.warning(`扫描失败：${e}，显示模拟数据`);
      // 模拟数据
      const mock: LargeFile[] = [
        { path: "/Users/zifang/Downloads/ubuntu-22.04.iso", size: 4.7 * 1024 * 1024 * 1024, modified: Date.now() / 1000 - 86400 * 3, is_dir: false },
        { path: "/Users/zifang/Movies/sample.mp4", size: 2.1 * 1024 * 1024 * 1024, modified: Date.now() / 1000 - 86400 * 7, is_dir: false },
        { path: "/Users/zifang/Library/Developer/Xcode/DerivedData", size: 1.8 * 1024 * 1024 * 1024, modified: Date.now() / 1000 - 86400, is_dir: true },
        { path: "/Users/zifang/Videos/screen-recording.mov", size: 950 * 1024 * 1024, modified: Date.now() / 1000 - 86400 * 14, is_dir: false },
        { path: "/Users/zifang/Documents/backup.zip", size: 680 * 1024 * 1024, modified: Date.now() / 1000 - 86400 * 30, is_dir: false },
        { path: "/Applications/Xcode.app", size: 32 * 1024 * 1024 * 1024, modified: Date.now() / 1000 - 86400 * 60, is_dir: true },
      ].filter((f) => f.size >= largeFileMinSize * 1024 * 1024);
      setLargeFiles(mock);
    } finally {
      setScanningLarge(false);
    }
  };

  // 加载启动项
  const loadStartupItems = async () => {
    setLoadingStartup(true);
    try {
      const items = await invoke<StartupItem[]>("get_startup_items_cmd");
      setStartupItems(items);
    } catch (e: any) {
      msgApi.warning(`加载失败：${e}`);
      // 模拟数据
      setStartupItems([
        { id: "1", name: "iTerm2", command: "/Applications/iTerm2.app", source: "Login Items", enabled: true, location: "macOS System Preferences" },
        { id: "2", name: "Docker Desktop", command: "/Applications/Docker.app", source: "LaunchAgent", enabled: true, location: "~/Library/LaunchAgents" },
        { id: "3", name: "Spotify", command: "/Applications/Spotify.app", source: "Login Items", enabled: true, location: "macOS System Preferences" },
        { id: "4", name: "Raycast", command: "/Applications/Raycast.app", source: "Login Items", enabled: true, location: "macOS System Preferences" },
      ]);
    } finally {
      setLoadingStartup(false);
    }
  };

  // 杀进程
  const handleKillProcess = async (pid: number) => {
    try {
      await invoke("kill_process", { pid });
      msgApi.success(`已结束进程 ${pid}`);
    } catch (e: any) {
      msgApi.warning(`结束失败：${e}`);
    }
  };

  // 刷新 DNS
  const flushDns = async () => {
    try {
      const result = await invoke<string>("flush_dns_cache");
      msgApi.success(result);
    } catch (e: any) {
      msgApi.warning(`刷新失败：${e}（可能需要管理员权限）`);
    }
  };

  // 进程数据
  const processColumns = [
    { title: "PID", dataIndex: "pid", key: "pid", width: 80 },
    { title: "进程名", dataIndex: "name", key: "name" },
    {
      title: "CPU",
      dataIndex: "cpu",
      key: "cpu",
      render: (value: number) => (
        <span style={{ color: getStatusColor(value, 80) }}>{value.toFixed(1)}%</span>
      ),
    },
    {
      title: "内存",
      dataIndex: "memory",
      key: "memory",
      render: (value: number) => (
        <span style={{ color: getStatusColor(value, 500) }}>{value.toFixed(1)} MB</span>
      ),
    },
    { title: "线程", dataIndex: "threads", key: "threads" },
    {
      title: "操作",
      key: "action",
      width: 100,
      render: (_: any, record: any) => (
        <Popconfirm
          title="确认结束此进程?"
          description={`将强制结束 PID ${record.pid}`}
          onConfirm={() => handleKillProcess(record.pid)}
        >
          <Button type="link" danger icon={<StopOutlined />} size="small">
            结束
          </Button>
        </Popconfirm>
      ),
    },
  ];

  const processData = [
    { pid: 1234, name: "z-biz-tool-sys", cpu: 2.5, memory: 120.5, threads: 12 },
    { pid: 2345, name: "Google Chrome", cpu: 15.2, memory: 1250.8, threads: 45 },
    { pid: 3456, name: "VSCode", cpu: 8.7, memory: 850.2, threads: 28 },
    { pid: 4567, name: "Docker Desktop", cpu: 5.3, memory: 650.5, threads: 32 },
  ];

  return (
    <ConfigProvider
      theme={{
        algorithm: darkMode ? theme.darkAlgorithm : theme.defaultAlgorithm,
      }}
    >
      {msgContext}
      <Layout style={{ height: "100vh" }}>
        <Header
          style={{
            background: cardBgGradient,
            padding: "0 24px",
            borderBottom: `1px solid var(--ant-color-border-secondary)`,
            display: "flex",
            justifyContent: "space-between",
            alignItems: "center",
          }}
        >
          <Space>
            <DashboardOutlined style={{ fontSize: "24px", background: brandGradient, WebkitBackgroundClip: "text", WebkitTextFillColor: "transparent" }} />
            <Title level={4} style={{ margin: 0, background: brandGradient, WebkitBackgroundClip: "text", WebkitTextFillColor: "transparent" }}>
              系统监控仪表盘
            </Title>
          </Space>
          <Space>
            <Text type="secondary">
              {systemInfo.hostname} | {systemInfo.os}
            </Text>
            <Switch
              checked={darkMode}
              onChange={setDarkMode}
              checkedChildren="🌙"
              unCheckedChildren="☀️"
            />
            <Tooltip title="刷新 DNS 缓存">
              <Button 
                icon={<SyncOutlined />} 
                onClick={flushDns}
                style={{ 
                  borderRadius: 6,
                  transition: "all 0.2s cubic-bezier(0.4, 0, 0.2, 1)",
                }}
              >
                刷新 DNS
              </Button>
            </Tooltip>
            <SettingOutlined style={{ fontSize: "16px", cursor: "pointer" }} />
          </Space>
        </Header>
        <Content style={{ padding: "16px", overflow: "auto" }}>
          <Tabs activeKey={activeTab} onChange={setActiveTab} size="large" style={{ background: cardBgGradient, borderRadius: 16, overflow: "hidden" }}>
            {/* ============ 概览 ============ */}
            <Tabs.TabPane tab="概览" key="overview">
              <Row gutter={[16, 16]} style={{ marginBottom: "16px" }}>
                <Col span={6}>
                  <Card 
                    size="small" 
                    className="monitor-card"
                    style={{ 
                      borderRadius: 12,
                      background: cardBgGradient,
                      transition: "all 0.3s cubic-bezier(0.4, 0, 0.2, 1)",
                    }}
                  >
                    <div className="monitor-card-title" style={{ background: brandGradient, WebkitBackgroundClip: "text", WebkitTextFillColor: "transparent" }}>
                      <DesktopOutlined /> CPU 使用率
                    </div>
                    <div className="monitor-card-value">
                      {monitorData[monitorData.length - 1]?.cpu.toFixed(1)}
                      <span className="monitor-card-unit">%</span>
                    </div>
                    <Progress
                      percent={monitorData[monitorData.length - 1]?.cpu}
                      strokeColor={getStatusColor(monitorData[monitorData.length - 1]?.cpu, 80)}
                      showInfo={false}
                    />
                    <Text type="secondary" style={{ fontSize: "12px" }}>
                      {systemInfo.cpuModel} ({systemInfo.cpuCores} 核心)
                    </Text>
                  </Card>
                </Col>
                <Col span={6}>
                  <Card 
                    size="small" 
                    className="monitor-card"
                    style={{ 
                      borderRadius: 12,
                      background: cardBgGradient,
                      transition: "all 0.3s cubic-bezier(0.4, 0, 0.2, 1)",
                    }}
                  >
                    <div className="monitor-card-title" style={{ background: brandGradient, WebkitBackgroundClip: "text", WebkitTextFillColor: "transparent" }}>
                      <CloudOutlined /> 内存使用
                    </div>
                    <div className="monitor-card-value">
                      {systemInfo.usedMemory.toFixed(1)}
                      <span className="monitor-card-unit">GB</span>
                    </div>
                    <Progress
                      percent={(systemInfo.usedMemory / systemInfo.totalMemory) * 100}
                      strokeColor={getStatusColor((systemInfo.usedMemory / systemInfo.totalMemory) * 100, 80)}
                      showInfo={false}
                    />
                    <Text type="secondary" style={{ fontSize: "12px" }}>
                      共 {systemInfo.totalMemory.toFixed(1)} GB
                    </Text>
                  </Card>
                </Col>
                <Col span={6}>
                  <Card 
                    size="small" 
                    className="monitor-card"
                    style={{ 
                      borderRadius: 12,
                      background: cardBgGradient,
                      transition: "all 0.3s cubic-bezier(0.4, 0, 0.2, 1)",
                    }}
                  >
                    <div className="monitor-card-title" style={{ background: brandGradient, WebkitBackgroundClip: "text", WebkitTextFillColor: "transparent" }}>
                      <HddOutlined /> 磁盘使用
                    </div>
                    <div className="monitor-card-value">
                      {systemInfo.diskUsed.toFixed(0)}
                      <span className="monitor-card-unit">GB</span>
                    </div>
                    <Progress
                      percent={(systemInfo.diskUsed / systemInfo.diskTotal) * 100}
                      strokeColor={getStatusColor((systemInfo.diskUsed / systemInfo.diskTotal) * 100, 90)}
                      showInfo={false}
                    />
                    <Text type="secondary" style={{ fontSize: "12px" }}>
                      共 {systemInfo.diskTotal.toFixed(0)} GB
                    </Text>
                  </Card>
                </Col>
                <Col span={6}>
                  <Card 
                    size="small" 
                    className="monitor-card"
                    style={{ 
                      borderRadius: 12,
                      background: cardBgGradient,
                      transition: "all 0.3s cubic-bezier(0.4, 0, 0.2, 1)",
                    }}
                  >
                    <div className="monitor-card-title" style={{ background: brandGradient, WebkitBackgroundClip: "text", WebkitTextFillColor: "transparent" }}>
                      <WifiOutlined /> 网络流量
                    </div>
                    <div className="monitor-card-value">
                      {monitorData[monitorData.length - 1]?.network.toFixed(0)}
                      <span className="monitor-card-unit">KB/s</span>
                    </div>
                    <div style={{ height: "8px" }} />
                    <Text type="secondary" style={{ fontSize: "12px" }}>
                      {systemInfo.networkInterfaces[0]?.ip}
                    </Text>
                  </Card>
                </Col>
              </Row>

              <Row gutter={[16, 16]}>
                <Col span={12}>
                  <Card 
                    size="small" 
                    title={
                      <span style={{ 
                        fontWeight: 600,
                        background: brandGradient,
                        WebkitBackgroundClip: "text",
                        WebkitTextFillColor: "transparent",
                      }}>CPU 使用率趋势</span>
                    }
                    className="monitor-card"
                    style={{ 
                      borderRadius: 12,
                      background: cardBgGradient,
                    }}
                  >
                    <ResponsiveContainer width="100%" height={200}>
                      <AreaChart data={monitorData}>
                        <CartesianGrid strokeDasharray="3 3" />
                        <XAxis dataKey="time" tick={{ fontSize: 12 }} />
                        <YAxis domain={[0, 100]} tick={{ fontSize: 12 }} />
                        <ChartTooltip />
                        <Area
                          type="monotone"
                          dataKey="cpu"
                          stroke="#667eea"
                          fill="url(#cpuGradient)"
                        />
                        <defs id="cpuGradient">
                          <linearGradient id="colorCpu" x1="0" y1="0" x2="0" y2="1">
                            <stop offset="5%" stopColor="#667eea" stopOpacity={0.8}/>
                            <stop offset="95%" stopColor="#667eea" stopOpacity={0}/>
                          </linearGradient>
                        </defs>
                      </AreaChart>
                    </ResponsiveContainer>
                  </Card>
                </Col>
                <Col span={12}>
                  <Card 
                    size="small" 
                    title={
                      <span style={{ 
                        fontWeight: 600,
                        background: brandGradient,
                        WebkitBackgroundClip: "text",
                        WebkitTextFillColor: "transparent",
                      }}>内存使用趋势</span>
                    }
                    className="monitor-card"
                    style={{ 
                      borderRadius: 12,
                      background: cardBgGradient,
                    }}
                  >
                    <ResponsiveContainer width="100%" height={200}>
                      <AreaChart data={monitorData}>
                        <CartesianGrid strokeDasharray="3 3" />
                        <XAxis dataKey="time" tick={{ fontSize: 12 }} />
                        <YAxis domain={[0, 100]} tick={{ fontSize: 12 }} />
                        <ChartTooltip />
                        <Area
                          type="monotone"
                          dataKey="memory"
                          stroke="#764ba2"
                          fill="url(#memoryGradient)"
                        />
                        <defs id="memoryGradient">
                          <linearGradient id="colorMemory" x1="0" y1="0" x2="0" y2="1">
                            <stop offset="5%" stopColor="#764ba2" stopOpacity={0.8}/>
                            <stop offset="95%" stopColor="#764ba2" stopOpacity={0}/>
                          </linearGradient>
                        </defs>
                      </AreaChart>
                    </ResponsiveContainer>
                  </Card>
                </Col>
              </Row>
            </Tabs.TabPane>

            {/* ============ 系统清理 🆕 ============ */}
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
                    <Button
                      type="primary"
                      icon={<SearchOutlined />}
                      onClick={scanJunk}
                      loading={scanning}
                    >
                      扫描垃圾
                    </Button>
                    <Button
                      danger
                      icon={<DeleteOutlined />}
                      onClick={doCleanup}
                      loading={cleaning}
                      disabled={!junkReport || selectedJunkIds.length === 0}
                    >
                      清理选中 ({selectedJunkIds.length})
                    </Button>
                  </Space>
                }
              >
                {!junkReport ? (
                  <div
                    style={{
                      textAlign: "center",
                      padding: "60px 0",
                      color: "var(--ant-color-text-secondary)",
                    }}
                  >
                    <ClearOutlined style={{ fontSize: 64, marginBottom: 16 }} />
                    <div style={{ fontSize: 16, marginBottom: 8 }}>点击"扫描垃圾"开始分析</div>
                    <div style={{ fontSize: 12 }}>
                      扫描缓存文件、临时文件、回收站等，识别可清理空间
                    </div>
                  </div>
                ) : (
                  <>
                    <Row gutter={16} style={{ marginBottom: 16 }}>
                      <Col span={8}>
                        <Statistic
                          title="可清理空间"
                          value={formatBytes(junkReport.total_size)}
                          valueStyle={{ color: "#52c41a", fontSize: 28 }}
                        />
                      </Col>
                      <Col span={8}>
                        <Statistic
                          title="可清理文件数"
                          value={junkReport.total_files.toLocaleString()}
                          prefix={<FileSearchOutlined />}
                        />
                      </Col>
                      <Col span={8}>
                        <Statistic
                          title="扫描耗时"
                          value={`${junkReport.scan_time_ms} ms`}
                          valueStyle={{ color: "#1890ff" }}
                        />
                      </Col>
                    </Row>

                    <Table
                      rowSelection={{
                        selectedRowKeys: selectedJunkIds,
                        onChange: (keys) => setSelectedJunkIds(keys as string[]),
                      }}
                      columns={[
                        {
                          title: "类别",
                          dataIndex: "name",
                          render: (name: string, record: JunkCategory) => (
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
                          dataIndex: "risk_level",
                          width: 100,
                          render: (level: string) => {
                            const labels: Record<string, string> = {
                              safe: "安全",
                              moderate: "中等",
                              risky: "高风险",
                            };
                            return <Tag color={getRiskColor(level)}>{labels[level] || level}</Tag>;
                          },
                        },
                        {
                          title: "文件数",
                          dataIndex: "file_count",
                          width: 120,
                          render: (n: number) => n.toLocaleString(),
                        },
                        {
                          title: "占用空间",
                          dataIndex: "size",
                          width: 150,
                          render: (size: number) => (
                            <span style={{ fontWeight: 600, color: "#52c41a" }}>
                              {formatBytes(size)}
                            </span>
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
                                  ...等 {paths.length} 个
                                </Text>
                              )}
                            </Space>
                          ),
                        },
                      ]}
                      dataSource={junkReport.categories.map((c) => ({ ...c, key: c.id }))}
                      pagination={false}
                      size="middle"
                    />
                  </>
                )}
              </Card>
            </Tabs.TabPane>

            {/* ============ 大文件扫描 🆕 ============ */}
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
                    <span>大文件扫描</span>
                  </Space>
                }
                extra={
                  <Space>
                    <span>最小大小 (MB):</span>
                    <InputNumber
                      min={1}
                      value={largeFileMinSize}
                      onChange={(v) => setLargeFileMinSize(v || 100)}
                      style={{ width: 120 }}
                    />
                    <Button
                      type="primary"
                      icon={<SearchOutlined />}
                      onClick={scanLargeFiles}
                      loading={scanningLarge}
                    >
                      扫描
                    </Button>
                  </Space>
                }
              >
                {largeFiles.length === 0 ? (
                  <div
                    style={{
                      textAlign: "center",
                      padding: "60px 0",
                      color: "var(--ant-color-text-secondary)",
                    }}
                  >
                    <FileSearchOutlined style={{ fontSize: 64, marginBottom: 16 }} />
                    <div>点击"扫描"开始查找大文件</div>
                  </div>
                ) : (
                  <Table
                    rowKey="path"
                    columns={[
                      {
                        title: "文件路径",
                        dataIndex: "path",
                        render: (p: string) => <Text code style={{ fontSize: 12 }}>{p}</Text>,
                      },
                      {
                        title: "大小",
                        dataIndex: "size",
                        width: 150,
                        render: (s: number) => (
                          <span style={{ fontWeight: 600 }}>{formatBytes(s)}</span>
                        ),
                        sorter: (a: LargeFile, b: LargeFile) => a.size - b.size,
                      },
                      {
                        title: "修改时间",
                        dataIndex: "modified",
                        width: 180,
                        render: (t: number) => dayjs.unix(t).format("YYYY-MM-DD HH:mm"),
                      },
                    ]}
                    dataSource={largeFiles}
                    pagination={{ pageSize: 20 }}
                  />
                )}
              </Card>
            </Tabs.TabPane>

            {/* ============ 启动项 🆕 ============ */}
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
                    <span>开机启动项管理</span>
                  </Space>
                }
                extra={
                  <Button
                    icon={<ReloadOutlined />}
                    onClick={loadStartupItems}
                    loading={loadingStartup}
                  >
                    刷新
                  </Button>
                }
              >
                <Table
                  rowKey="id"
                  columns={[
                    {
                      title: "名称",
                      dataIndex: "name",
                      render: (n: string) => <strong>{n}</strong>,
                    },
                    {
                      title: "来源",
                      dataIndex: "source",
                      width: 150,
                      render: (s: string) => <Tag color="blue">{s}</Tag>,
                    },
                    {
                      title: "命令/路径",
                      dataIndex: "command",
                      render: (c: string) => (
                        <Text code style={{ fontSize: 12 }}>{c}</Text>
                      ),
                    },
                    {
                      title: "状态",
                      dataIndex: "enabled",
                      width: 100,
                      render: (e: boolean) =>
                        e ? (
                          <Tag color="success">已启用</Tag>
                        ) : (
                          <Tag>已禁用</Tag>
                        ),
                    },
                    {
                      title: "操作",
                      key: "action",
                      width: 120,
                      render: () => (
                        <Space size="small">
                          <Button size="small" disabled>
                            禁用
                          </Button>
                          <Button size="small" danger disabled>
                            删除
                          </Button>
                        </Space>
                      ),
                    },
                  ]}
                  dataSource={startupItems}
                  loading={loadingStartup}
                  pagination={false}
                />
              </Card>
            </Tabs.TabPane>

            {/* ============ 进程 ============ */}
            <Tabs.TabPane tab="进程" key="processes">
              <Card title="进程列表" className="monitor-card">
                <Table
                  columns={processColumns}
                  dataSource={processData}
                  size="small"
                  pagination={{ pageSize: 20 }}
                />
              </Card>
            </Tabs.TabPane>

            {/* ============ 网络 ============ */}
            <Tabs.TabPane tab="网络" key="network">
              <Card title="网络接口" className="monitor-card">
                <Table
                  columns={[
                    { title: "接口", dataIndex: "name", key: "name" },
                    { title: "IP 地址", dataIndex: "ip", key: "ip" },
                    { title: "MAC 地址", dataIndex: "mac", key: "mac" },
                    {
                      title: "速率",
                      dataIndex: "speed",
                      key: "speed",
                      render: (value: number) => `${value} Mbps`,
                    },
                  ]}
                  dataSource={systemInfo.networkInterfaces}
                  size="small"
                  pagination={false}
                />
              </Card>
            </Tabs.TabPane>

            {/* ============ 系统信息 ============ */}
            <Tabs.TabPane tab="系统信息" key="system">
              <Card title="系统信息" className="monitor-card">
                <Row gutter={[16, 16]}>
                  <Col span={12}>
                    <Statistic title="主机名" value={systemInfo.hostname} />
                  </Col>
                  <Col span={12}>
                    <Statistic title="操作系统" value={systemInfo.os} />
                  </Col>
                  <Col span={12}>
                    <Statistic title="内核版本" value={systemInfo.kernel} />
                  </Col>
                  <Col span={12}>
                    <Statistic title="运行时间" value={systemInfo.uptime} />
                  </Col>
                  <Col span={12}>
                    <Statistic title="CPU 型号" value={systemInfo.cpuModel} />
                  </Col>
                  <Col span={12}>
                    <Statistic title="CPU 核心数" value={systemInfo.cpuCores} />
                  </Col>
                  <Col span={12}>
                    <Statistic
                      title="总内存"
                      value={systemInfo.totalMemory}
                      suffix="GB"
                    />
                  </Col>
                  <Col span={12}>
                    <Statistic
                      title="已用内存"
                      value={systemInfo.usedMemory}
                      suffix="GB"
                      valueStyle={{
                        color: getStatusColor(
                          (systemInfo.usedMemory / systemInfo.totalMemory) * 100,
                          80
                        ),
                      }}
                    />
                  </Col>
                </Row>
              </Card>
            </Tabs.TabPane>
          </Tabs>
        </Content>
      </Layout>
    </ConfigProvider>
  );
}

export default App;