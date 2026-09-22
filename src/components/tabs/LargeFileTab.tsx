import { Button, Card, InputNumber, Space, Table, Typography } from "antd";
import { FileSearchOutlined, SearchOutlined } from "@ant-design/icons";
import dayjs from "dayjs";
import type { LargeFile } from "../../ipc_contract";
import { formatBytes } from "../../lib/format";

const { Text } = Typography;

interface Props {
  files: LargeFile[];
  minSizeMb: number;
  onMinSizeChange: (value: number) => void;
  scanning: boolean;
  onScan: () => void;
}

export function LargeFileTab({ files, minSizeMb, onMinSizeChange, scanning, onScan }: Props) {
  return (
    <Card
      title={
        <Space>
          <FileSearchOutlined style={{ color: "#faad14" }} />
          <span>大文件扫描（用户主目录之下）</span>
        </Space>
      }
      extra={
        <Space>
          <span>最小大小 (MB):</span>
          <InputNumber
            min={1}
            value={minSizeMb}
            onChange={(v) => onMinSizeChange(v || 100)}
            style={{ width: 120 }}
          />
          <Button type="primary" icon={<SearchOutlined />} onClick={onScan} loading={scanning}>
            扫描
          </Button>
        </Space>
      }
    >
      {files.length === 0 ? (
        <div
          style={{
            textAlign: "center",
            padding: "60px 0",
            color: "var(--ant-color-text-secondary)",
          }}
        >
          <FileSearchOutlined style={{ fontSize: 64, marginBottom: 16 }} />
          <div>点击“扫描”开始查找大文件</div>
        </div>
      ) : (
        <Table<LargeFile>
          rowKey="path"
          columns={[
            {
              title: "文件路径",
              dataIndex: "path",
              ellipsis: true,
              render: (p: string) => (
                <Text code style={{ fontSize: 12 }}>
                  {p}
                </Text>
              ),
            },
            {
              title: "大小",
              dataIndex: "sizeBytes",
              width: 150,
              sorter: (a, b) => a.sizeBytes - b.sizeBytes,
              defaultSortOrder: "descend" as const,
              render: (s: number) => <span style={{ fontWeight: 600 }}>{formatBytes(s)}</span>,
            },
            {
              title: "修改时间",
              dataIndex: "modifiedSeconds",
              width: 180,
              render: (t: number) => (t > 0 ? dayjs.unix(t).format("YYYY-MM-DD HH:mm") : "—"),
            },
          ]}
          dataSource={files}
          pagination={{ pageSize: 20 }}
        />
      )}
    </Card>
  );
}
