import { Alert, Card, Col, Row, Statistic } from "antd";
import type { MetricsSnapshot, StaticInfo } from "../../ipc_contract";
import { formatGb, formatUptime, usageColor } from "../../lib/format";

export function SystemInfoTab({
  staticInfo,
  snapshot,
}: {
  staticInfo: StaticInfo | null;
  snapshot: MetricsSnapshot | null;
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
    </Card>
  );
}
