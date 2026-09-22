import { Alert, Button, Card, Col, Divider, Row, Space, Statistic, Table, Tag, Typography } from "antd";
import type { MetricsSnapshot, StaticInfo, ThermalReport, ThermalSensor } from "../../ipc_contract";
import { formatGb, formatUptime, usageColor } from "../../lib/format";
import { SEVERITY_COLOR, formatSensorLimit, formatSensorValue, severityText } from "../../lib/thermal";

const { Text } = Typography;

const SENSOR_COLUMNS = [
  { title: "传感器", dataIndex: "label", key: "label" },
  {
    title: "读数",
    key: "value",
    render: (_: unknown, sensor: ThermalSensor) => (
      <Text
        style={{
          color: SEVERITY_COLOR[sensor.severity],
          fontVariantNumeric: "tabular-nums",
        }}
      >
        {formatSensorValue(sensor)}
      </Text>
    ),
  },
  {
    title: "硬件上限",
    key: "limit",
    render: (_: unknown, sensor: ThermalSensor) => (
      <Text type="secondary">{formatSensorLimit(sensor)}</Text>
    ),
  },
  {
    title: "状态",
    key: "severity",
    render: (_: unknown, sensor: ThermalSensor) => (
      <Tag color={SEVERITY_COLOR[sensor.severity] ?? "default"}>{severityText(sensor)}</Tag>
    ),
  },
  {
    title: "来源",
    dataIndex: "source",
    key: "source",
    render: (source: string) => (
      <Text type="secondary" style={{ fontSize: 12 }}>
        {source}
      </Text>
    ),
  },
];

export function SystemInfoTab({
  staticInfo,
  snapshot,
  thermal,
  thermalLoading,
  onRefreshThermal,
  onTransferPrefs,
}: {
  staticInfo: StaticInfo | null;
  snapshot: MetricsSnapshot | null;
  /** null = 还没查过；空 sensors + reason = 查过但这台机器给不出读数，两种状态要分开显示 */
  thermal: ThermalReport | null;
  thermalLoading: boolean;
  onRefreshThermal: () => void;
  /** 打开偏好导入/导出弹层（T5-11）。入口放这里，弹层本体在 App 的树尾 */
  onTransferPrefs: () => void;
}) {
  const memory = snapshot?.memory;
  return (
    <Card title="系统信息" className="monitor-card">
      {staticInfo ? (
        <Row gutter={[16, 16]}>
          <Col span={12}>
            <Statistic title="主机名" value={staticInfo.hostname} />
          </Col>
          <Col span={12}>
            <Statistic
              title="操作系统"
              value={`${staticInfo.osName} ${staticInfo.osVersion}`}
            />
          </Col>
          <Col span={12}>
            <Statistic title="内核版本" value={staticInfo.kernelVersion} />
          </Col>
          <Col span={12}>
            <Statistic
              title="运行时间"
              value={snapshot ? formatUptime(snapshot.uptimeSeconds) : "—"}
            />
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
              styles={{ content: { color: memory ? usageColor(memory.usagePercent, 80) : undefined } }}
            />
          </Col>
          <Col span={12}>
            <Statistic
              title="平台 / 架构"
              value={`${staticInfo.platform} / ${staticInfo.arch}`}
            />
          </Col>
          <Col span={12}>
            <Statistic title="当前用户" value={staticInfo.currentUserName ?? "—"} />
          </Col>
        </Row>
      ) : (
        <Alert type="info" showIcon title="正在读取系统信息…" />
      )}

      <Divider style={{ margin: "16px 0" }} />

      <Space align="center" wrap style={{ marginBottom: 8 }}>
        <Text strong>温度 / 风扇</Text>
        <Button size="small" loading={thermalLoading} onClick={onRefreshThermal}>
          重新读取
        </Button>
      </Space>
      {thermal === null ? (
        <Alert
          type="info"
          showIcon
          title={thermalLoading ? "正在读取传感器…" : "还没有拿到这台机器的传感器读数"}
          description="读数只在 Linux 的 /sys 上免提权可得，macOS 与 Windows 要走提权的接口 —— 本应用内不提权，也不会先摆一个 0 占位。"
        />
      ) : thermal.sensors.length === 0 ? (
        <Alert
          type="warning"
          showIcon
          title={thermal.reason ?? "后端没有给出读数，也没有给出原因"}
          description="这一项没有读数就是真的没有读数：不用机型平均值、不用估算值补位。"
        />
      ) : (
        <Table<ThermalSensor>
          size="small"
          rowKey="source"
          pagination={false}
          columns={SENSOR_COLUMNS}
          dataSource={thermal.sensors}
        />
      )}

      <Divider style={{ margin: "16px 0" }} />
      <Space align="center" wrap>
        <Text type="secondary">
          这几项界面设置（主题、采集间隔、趋势窗口、默认页签、告警阈值）可以导成一份 JSON 备份或换机导入。
        </Text>
        <Button size="small" onClick={onTransferPrefs}>
          偏好导入 / 导出
        </Button>
      </Space>
    </Card>
  );
}
