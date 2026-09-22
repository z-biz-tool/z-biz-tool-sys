import { Card, Progress, Table, Typography } from "antd";
import type { MetricsSnapshot } from "../../ipc_contract";
import { formatBytes, formatRate, usageColor } from "../../lib/format";

const { Text } = Typography;

type DiskRow = MetricsSnapshot["disks"][number];

/** 后端说"这一项没有读数"时界面唯一的画法。0 是读数，不是缺测。 */
const MISSING = "—";

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
            // `available: false` 是"这块分区的容量读不出来"，不是"容量 0 / 用了 0 %"。
            // 后端对这种分区给的 `usagePercent` 是 0.0（类型上不是 Option），所以必须由这一列
            // 把它显示成 `—`，否则 FI-04 里断掉的挂载点会在表里装成一块全空的盘。
            render: (v: number, disk: DiskRow) => (disk.available ? formatBytes(v, 0) : MISSING),
          },
          {
            title: "已用",
            dataIndex: "usedBytes",
            key: "used",
            width: 120,
            render: (v: number, disk: DiskRow) => (disk.available ? formatBytes(v, 0) : MISSING),
          },
          {
            title: "可用",
            dataIndex: "availableBytes",
            key: "free",
            width: 120,
            render: (v: number, disk: DiskRow) => (disk.available ? formatBytes(v, 0) : MISSING),
          },
          {
            title: "使用率",
            dataIndex: "usagePercent",
            key: "usagePercent",
            width: 160,
            render: (v: number, disk: DiskRow) =>
              disk.available ? (
                <Progress
                  percent={Number(v.toFixed(1))}
                  size="small"
                  strokeColor={usageColor(v, 90)}
                />
              ) : (
                <Text type="secondary">{MISSING}</Text>
              ),
          },
          {
            title: "读取",
            dataIndex: "readBytesPerSec",
            key: "read",
            width: 110,
            render: (v: number | null) => formatRate(v),
          },
          {
            title: "写入",
            dataIndex: "writeBytesPerSec",
            key: "write",
            width: 110,
            render: (v: number | null) => formatRate(v),
          },
        ]}
        dataSource={snapshot?.disks ?? []}
        size="small"
        pagination={false}
        locale={{ emptyText: "等待采集首帧…" }}
      />
      <Text type="secondary" style={{ fontSize: 12 }}>
        读/写速率取自块设备计数（IOKit），是最近一个磁盘采样窗口（默认 10 s）的均值，
        窗口内各帧沿用同一数值；拿不到计数的挂载点显示 —，空闲显示 0 B/s。
        同一物理盘的多个 APFS 卷（如 / 与 /System/Volumes/Data）会显示相同计数。
      </Text>
    </Card>
  );
}
