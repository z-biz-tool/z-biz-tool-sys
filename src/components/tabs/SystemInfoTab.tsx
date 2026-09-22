import { Alert, Button, Card, Col, Divider, Row, Space, Statistic, Typography } from "antd";
import type { MetricsSnapshot, StaticInfo } from "../../ipc_contract";
import { formatGb, formatUptime, usageColor } from "../../lib/format";

const { Text } = Typography;

export function SystemInfoTab({
  staticInfo,
  snapshot,
  onTransferPrefs,
}: {
  staticInfo: StaticInfo | null;
  snapshot: MetricsSnapshot | null;
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
