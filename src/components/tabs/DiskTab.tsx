import { Card, Progress, Table, Typography } from "antd";
import type { MetricsSnapshot } from "../../ipc_contract";
import { formatBytes, usageColor } from "../../lib/format";

const { Text } = Typography;

export function DiskTab({ snapshot }: { snapshot: MetricsSnapshot | null }) {
  return (
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
          {
            title: "容量",
            dataIndex: "totalBytes",
            key: "total",
            width: 120,
            render: (v: number) => formatBytes(v, 0),
          },
          {
            title: "已用",
            dataIndex: "usedBytes",
            key: "used",
            width: 120,
            render: (v: number) => formatBytes(v, 0),
          },
          {
            title: "可用",
            dataIndex: "availableBytes",
            key: "free",
            width: 120,
            render: (v: number) => formatBytes(v, 0),
          },
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
  );
}
