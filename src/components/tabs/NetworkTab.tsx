import { Card, Table, Tag, Typography } from "antd";
import type { MetricsSnapshot } from "../../ipc_contract";
import { formatRate } from "../../lib/format";

const { Text } = Typography;

export function NetworkTab({ snapshot }: { snapshot: MetricsSnapshot | null }) {
  return (
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
  );
}
