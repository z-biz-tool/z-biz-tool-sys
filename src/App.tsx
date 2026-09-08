import { useState, useEffect } from "react";
import { ConfigProvider, theme, Layout, Card, Row, Col, Statistic, Switch, Space, Typography, Tabs, Table, Progress } from "antd";
import {
  DashboardOutlined,
  DesktopOutlined,
  CloudOutlined,
  HddOutlined,
  WifiOutlined,
  ThunderboltOutlined,
  ReloadOutlined,
  FullscreenOutlined,
  SettingOutlined,
} from "@ant-design/icons";
import { LineChart, Line, XAxis, YAxis, CartesianGrid, Tooltip, ResponsiveContainer, AreaChart, Area } from "recharts";
import dayjs from "dayjs";

const { Header, Content } = Layout;
const { Title, Text } = Typography;

// 模拟数据生成
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

// 系统信息类型
interface SystemInfo {
  hostname: string;
  os: string;
  kernel: string;
  uptime: string;
  cpuModel: string;
  cpuCores: number;
  totalMemory: number;
  usedMemory: number;
  diskTotal: number;
  diskUsed: number;
  networkInterfaces: Array<{
    name: string;
    ip: string;
    mac: string;
    speed: number;
  }>;
}

// 模拟系统信息
const mockSystemInfo: SystemInfo = {
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
  const [systemInfo, setSystemInfo] = useState<SystemInfo>(mockSystemInfo);
  const [monitorData, setMonitorData] = useState(generateMockData(60));
  const [refreshInterval, setRefreshInterval] = useState(1);
  const [activeTab, setActiveTab] = useState("overview");

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

  // 进程数据（模拟）
  const processColumns = [
    {
      title: "PID",
      dataIndex: "pid",
      key: "pid",
    },
    {
      title: "进程名",
      dataIndex: "name",
      key: "name",
    },
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
    {
      title: "线程",
      dataIndex: "threads",
      key: "threads",
    },
  ];

  const processData = [
    { pid: 1234, name: "z-biz-tool-sys", cpu: 2.5, memory: 120.5, threads: 12 },
    { pid: 2345, name: "Google Chrome", cpu: 15.2, memory: 1250.8, threads: 45 },
    { pid: 3456, name: "VSCode", cpu: 8.7, memory: 850.2, threads: 28 },
    { pid: 4567, name: "Docker Desktop", cpu: 5.3, memory: 650.5, threads: 32 },
    { pid: 5678, name: "iTerm2", cpu: 1.2, memory: 85.3, threads: 8 },
  ];

  return (
    <ConfigProvider
      theme={{
        algorithm: darkMode ? theme.darkAlgorithm : theme.defaultAlgorithm,
      }}
    >
      <Layout style={{ height: "100vh" }}>
        <Header
          style={{
            background: darkMode ? "#141414" : "#fff",
            padding: "0 24px",
            borderBottom: "1px solid #f0f0f0",
            display: "flex",
            justifyContent: "space-between",
            alignItems: "center",
          }}
        >
          <Space>
            <DashboardOutlined style={{ fontSize: "24px", color: "#1890ff" }} />
            <Title level={4} style={{ margin: 0 }}>
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
            <SettingOutlined style={{ fontSize: "16px", cursor: "pointer" }} />
          </Space>
        </Header>
        <Content style={{ padding: "16px", overflow: "auto" }}>
          <Tabs activeKey={activeTab} onChange={setActiveTab}>
            <Tabs.TabPane tab="概览" key="overview">
              {/* 系统概览卡片 */}
              <Row gutter={[16, 16]} style={{ marginBottom: "16px" }}>
                <Col span={6}>
                  <Card size="small" className="monitor-card">
                    <div className="monitor-card-title">
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
                  <Card size="small" className="monitor-card">
                    <div className="monitor-card-title">
                      <CloudOutlined /> 内存使用
                    </div>
                    <div className="monitor-card-value">
                      {systemInfo.usedMemory.toFixed(1)}
                      <span className="monitor-card-unit">GB</span>
                    </div>
                    <Progress
                      percent={(systemInfo.usedMemory / systemInfo.totalMemory) * 100}
                      strokeColor={getStatusColor(
                        (systemInfo.usedMemory / systemInfo.totalMemory) * 100,
                        80
                      )}
                      showInfo={false}
                    />
                    <Text type="secondary" style={{ fontSize: "12px" }}>
                      共 {systemInfo.totalMemory.toFixed(1)} GB
                    </Text>
                  </Card>
                </Col>
                <Col span={6}>
                  <Card size="small" className="monitor-card">
                    <div className="monitor-card-title">
                      <HddOutlined /> 磁盘使用
                    </div>
                    <div className="monitor-card-value">
                      {systemInfo.diskUsed.toFixed(0)}
                      <span className="monitor-card-unit">GB</span>
                    </div>
                    <Progress
                      percent={(systemInfo.diskUsed / systemInfo.diskTotal) * 100}
                      strokeColor={getStatusColor(
                        (systemInfo.diskUsed / systemInfo.diskTotal) * 100,
                        90
                      )}
                      showInfo={false}
                    />
                    <Text type="secondary" style={{ fontSize: "12px" }}>
                      共 {systemInfo.diskTotal.toFixed(0)} GB
                    </Text>
                  </Card>
                </Col>
                <Col span={6}>
                  <Card size="small" className="monitor-card">
                    <div className="monitor-card-title">
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

              {/* 实时图表 */}
              <Row gutter={[16, 16]}>
                <Col span={12}>
                  <Card size="small" title="CPU 使用率趋势" className="monitor-card">
                    <ResponsiveContainer width="100%" height={200}>
                      <AreaChart data={monitorData}>
                        <CartesianGrid strokeDasharray="3 3" />
                        <XAxis dataKey="time" tick={{ fontSize: 12 }} />
                        <YAxis domain={[0, 100]} tick={{ fontSize: 12 }} />
                        <Tooltip />
                        <Area
                          type="monotone"
                          dataKey="cpu"
                          stroke="#1890ff"
                          fill="#1890ff"
                          fillOpacity={0.2}
                        />
                      </AreaChart>
                    </ResponsiveContainer>
                  </Card>
                </Col>
                <Col span={12}>
                  <Card size="small" title="内存使用趋势" className="monitor-card">
                    <ResponsiveContainer width="100%" height={200}>
                      <AreaChart data={monitorData}>
                        <CartesianGrid strokeDasharray="3 3" />
                        <XAxis dataKey="time" tick={{ fontSize: 12 }} />
                        <YAxis domain={[0, 100]} tick={{ fontSize: 12 }} />
                        <Tooltip />
                        <Area
                          type="monotone"
                          dataKey="memory"
                          stroke="#52c41a"
                          fill="#52c41a"
                          fillOpacity={0.2}
                        />
                      </AreaChart>
                    </ResponsiveContainer>
                  </Card>
                </Col>
              </Row>
            </Tabs.TabPane>

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