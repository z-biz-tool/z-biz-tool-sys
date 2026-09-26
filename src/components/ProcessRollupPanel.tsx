import { useMemo } from "react";
import { Alert, Button, Card, Space, Table, Typography } from "antd";
import { ReloadOutlined } from "@ant-design/icons";
import type { ProcessRollup } from "../ipc_contract";
import { formatBytes } from "../lib/format";

const { Text } = Typography;

interface Props {
  /** null = 这一轮没读到（没试过或那一次失败）；空数组 = 读到了但没有任何进程行 */
  rollup: ProcessRollup[] | null;
  loading: boolean;
  onRefresh: () => void;
  onOpenDetail: (pid: number) => void;
}

/**
 * 运行时归类（T6-01）："怎么跑了这么多 java 进程"这类问题以前只能人肉把几百行加总。
 *
 * 这里的内存列刻意不校正成"看起来不超过物理内存"：它是各进程常驻内存之和，
 * 共享页会重复计入，所以偏大。口径写在界面上，而不是把数字改小。
 */
export function ProcessRollupPanel({ rollup, loading, onRefresh, onOpenDetail }: Props) {
  const totalBytes = useMemo(
    () => (rollup ?? []).reduce((sum, bucket) => sum + bucket.memoryBytes, 0),
    [rollup]
  );
  const totalCount = useMemo(
    () => (rollup ?? []).reduce((sum, bucket) => sum + bucket.processCount, 0),
    [rollup]
  );

  const columns = [
    {
      title: "类别",
      dataIndex: "category",
      key: "category",
      width: 130,
      render: (value: string) => <Text strong>{value}</Text>,
    },
    { title: "进程数", dataIndex: "processCount", key: "processCount", width: 90 },
    {
      title: "内存合计",
      dataIndex: "memoryBytes",
      key: "memoryBytes",
      width: 120,
      render: (value: number) => formatBytes(value, 1),
    },
    {
      title: "占 ΣRSS",
      key: "share",
      width: 90,
      render: (_: unknown, bucket: ProcessRollup) =>
        totalBytes > 0 ? `${((bucket.memoryBytes / totalBytes) * 100).toFixed(1)} %` : "—",
    },
    {
      title: "已脱离启动者",
      dataIndex: "detachedCount",
      key: "detachedCount",
      width: 120,
      render: (value: number) => (value > 0 ? <Text>{value}</Text> : <Text type="secondary">0</Text>),
    },
    {
      title: "这一类里最大的",
      key: "top",
      render: (_: unknown, bucket: ProcessRollup) =>
        bucket.topPid === null ? (
          <Text type="secondary">—</Text>
        ) : (
          <Button
            type="link"
            size="small"
            style={{ padding: 0, height: "auto" }}
            onClick={() => onOpenDetail(bucket.topPid as number)}
          >
            {bucket.topName ?? `PID ${bucket.topPid}`} · {bucket.topPid}
          </Button>
        ),
    },
  ];

  return (
    <Card
      title={`运行时归类（${rollup ? `${totalCount} 个进程` : "尚未读取"}）`}
      className="monitor-card"
      extra={
        <Button icon={<ReloadOutlined />} size="small" loading={loading} onClick={onRefresh}>
          重新观察
        </Button>
      }
    >
      {rollup === null ? (
        <Alert
          type="info"
          showIcon
          title="还没读到归类数据。空白不代表机器没在跑东西，只代表这一轮没采到——点「重新观察」再试。"
        />
      ) : rollup.length === 0 ? (
        <Alert
          type="warning"
          showIcon
          title="归类返回空表：一次全量枚举没拿到任何进程行。这几乎总是采集失败，不是「机器上没进程」。"
        />
      ) : (
        <>
          <Table<ProcessRollup>
            rowKey="category"
            columns={columns}
            dataSource={rollup}
            size="small"
            pagination={false}
            scroll={{ x: 760 }}
          />
          <Space direction="vertical" size={2} style={{ marginTop: 10 }}>
            <Text type="secondary" style={{ fontSize: 12 }}>
              口径：一次全量枚举，不是进程表当前这一页；内存是各进程常驻内存之和，跨进程共享页会重复计入，
              因此这一列的合计比实际物理占用偏大。
            </Text>
            <Text type="secondary" style={{ fontSize: 12 }}>
              "已脱离启动者"只说明父进程已经是 launchd（原始启动者退出过）；launchd 托管的系统服务
              同样满足这一条，该不该收手要结合属主与运行时长看。
            </Text>
          </Space>
        </>
      )}
    </Card>
  );
}
