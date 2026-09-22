import { Button, Card, Space, Table, Tag, Tooltip, Typography } from "antd";
import { ReloadOutlined, RocketOutlined } from "@ant-design/icons";
import type { StartupItem } from "../../ipc_contract";

const { Text } = Typography;

interface Props {
  items: StartupItem[];
  loading: boolean;
  onReload: () => void;
}

export function StartupTab({ items, loading, onReload }: Props) {
  return (
    <Card
      title={
        <Space>
          <RocketOutlined style={{ color: "#722ed1" }} />
          <span>开机启动项（只读枚举）</span>
        </Space>
      }
      extra={
        <Button icon={<ReloadOutlined />} onClick={onReload} loading={loading}>
          刷新
        </Button>
      }
    >
      <Table<StartupItem>
        rowKey="id"
        columns={[
          { title: "名称", dataIndex: "name", render: (n: string) => <strong>{n}</strong> },
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
              <Text code style={{ fontSize: 12 }}>
                {c}
              </Text>
            ),
          },
          {
            title: "状态",
            dataIndex: "enabled",
            width: 100,
            render: (e: boolean) => (e ? <Tag color="success">已启用</Tag> : <Tag>已禁用</Tag>),
          },
          {
            title: "操作",
            key: "action",
            width: 140,
            render: () => (
              <Tooltip title="禁用/删除启动项尚未实现（T3-08），不会执行任何写操作">
                <Space size="small">
                  <Button size="small" disabled>
                    禁用
                  </Button>
                  <Button size="small" danger disabled>
                    删除
                  </Button>
                </Space>
              </Tooltip>
            ),
          },
        ]}
        dataSource={items}
        loading={loading}
        pagination={false}
        locale={{ emptyText: "点击“刷新”读取系统启动项" }}
      />
    </Card>
  );
}
