import { Alert, Button, Card, Space, Table, Tag, Typography } from "antd";
import { ReloadOutlined } from "@ant-design/icons";
import type { ListeningReport, ListeningSocket } from "../ipc_contract";

const { Text } = Typography;

interface Props {
  /** null = 这一轮没读到（命令失败或还没试）；非 null 时 sockets 可以为空 */
  report: ListeningReport | null;
  loading: boolean;
  onRefresh: () => void;
  onOpenDetail: (pid: number) => void;
}

/**
 * 监听端口反查进程（T6-01）："8201 被谁占着"以前只能开终端跑 `lsof`。
 *
 * 空表一定带着原因：没有 lsof、没权限、真的一个都没有，这三种情况的下一步完全不同，
 * 糊成同一个空列表等于没说。
 */
export function ListeningPortsPanel({ report, loading, onRefresh, onOpenDetail }: Props) {
  const columns = [
    { title: "端口", dataIndex: "port", key: "port", width: 90 },
    {
      title: "协议",
      dataIndex: "protocol",
      key: "protocol",
      width: 80,
      render: (value: string) => <Tag>{value.toUpperCase()}</Tag>,
    },
    {
      title: "监听地址",
      dataIndex: "address",
      key: "address",
      width: 180,
      render: (value: string) => (
        <Text code style={{ fontSize: 12 }}>
          {value}
        </Text>
      ),
    },
    {
      title: "进程",
      key: "process",
      render: (_: unknown, socket: ListeningSocket) => (
        <Button
          type="link"
          size="small"
          style={{ padding: 0, height: "auto" }}
          onClick={() => onOpenDetail(socket.pid)}
        >
          {socket.processName ?? `PID ${socket.pid}（本轮进程表里没有）`}
        </Button>
      ),
    },
    { title: "PID", dataIndex: "pid", key: "pid", width: 90 },
  ];

  return (
    <Card
      title={`监听端口（TCP LISTEN · ${report ? `${report.sockets.length} 个` : "尚未读取"}）`}
      className="monitor-card"
      extra={
        <Button icon={<ReloadOutlined />} size="small" loading={loading} onClick={onRefresh}>
          重新读取
        </Button>
      }
    >
      {report === null ? (
        <Alert
          type="info"
          showIcon
          title="还没读到监听列表。空白不代表这台机器没有端口在听，只代表这一轮没采到。"
        />
      ) : report.sockets.length === 0 ? (
        <Alert
          type="warning"
          showIcon
          title={report.reason ?? "没有返回任何监听，也没有给出原因。"}
        />
      ) : (
        <>
          <Table<ListeningSocket>
            rowKey={(row) => `${row.protocol}:${row.address}:${row.port}:${row.pid}`}
            columns={columns}
            dataSource={report.sockets}
            size="small"
            pagination={false}
            scroll={{ x: 700, y: 320 }}
          />
          <Space direction="vertical" size={2} style={{ marginTop: 10 }}>
            <Text type="secondary" style={{ fontSize: 12 }}>
              来源是一次 <Text code>lsof -nP -iTCP -sTCP:LISTEN</Text>，只在点「重新读取」时才跑，
              不在 1 秒的采集循环里。
            </Text>
            <Text type="secondary" style={{ fontSize: 12 }}>
              非提权进程只能看到<Text strong>当前用户</Text>的 socket，系统服务那一栏可能整片是空的
              ——这是权限边界，不是「它们没在听」。
            </Text>
          </Space>
        </>
      )}
    </Card>
  );
}
