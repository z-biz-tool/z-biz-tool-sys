import {
  Alert,
  Button,
  Card,
  Col,
  Progress,
  Row,
  Space,
  Statistic,
  Table,
  Tag,
  Typography,
} from "antd";
import {
  ClearOutlined,
  DeleteOutlined,
  FileSearchOutlined,
  SearchOutlined,
} from "@ant-design/icons";
import type { JunkCategory, JunkReport, ScanProgress } from "../../ipc_contract";
import { formatBytes } from "../../lib/format";
import { RISK_LABELS, riskColor } from "../../lib/ui";

const { Text } = Typography;

interface Props {
  report: JunkReport | null;
  selectedIds: string[];
  onSelectedIdsChange: (ids: string[]) => void;
  scanning: boolean;
  cleaning: boolean;
  /** 后端逐目录推送的进度帧；null 表示还没有收到 */
  progress: ScanProgress | null;
  cancelRequested: boolean;
  onScan: () => void;
  onCancelScan: () => void;
  onRequestCleanup: () => void;
}

export function CleanupTab({
  report,
  selectedIds,
  onSelectedIdsChange,
  scanning,
  cleaning,
  progress,
  cancelRequested,
  onScan,
  onCancelScan,
  onRequestCleanup,
}: Props) {
  // 百分比只按"扫完的目录数 / 白名单放行的目录数"算，不预估时间也不补合成点。
  const percent =
    progress && progress.totalPaths > 0
      ? Math.min(100, Math.round((progress.scannedPaths / progress.totalPaths) * 100))
      : 0;

  return (
    <Card
      title={
        <Space>
          <ClearOutlined style={{ color: "#52c41a" }} />
          <span>垃圾文件扫描与清理</span>
        </Space>
      }
      extra={
        <Space>
          {/* antd 的 loading 按钮仍可点击，不 disabled 就会重复下发扫描 */}
          <Button
            type="primary"
            icon={<SearchOutlined />}
            onClick={onScan}
            loading={scanning}
            disabled={scanning}
          >
            扫描垃圾
          </Button>
          {scanning && (
            <Button danger onClick={onCancelScan} disabled={cancelRequested}>
              {cancelRequested ? "取消中…" : "取消扫描"}
            </Button>
          )}
          <Button
            danger
            icon={<DeleteOutlined />}
            loading={cleaning}
            disabled={!report || selectedIds.length === 0 || scanning}
            onClick={onRequestCleanup}
          >
            清理选中 ({selectedIds.length})
          </Button>
        </Space>
      }
    >
      {scanning && (
        <div style={{ marginBottom: 16 }}>
          <Progress percent={percent} status={cancelRequested ? "exception" : "active"} />
          <Space orientation="vertical" size={2}>
            <Text type="secondary" style={{ fontSize: 12 }}>
              {progress
                ? `已扫描 ${progress.scannedPaths} / ${progress.totalPaths} 个目录 · 当前目录 ${progress.currentPathFiles.toLocaleString()} 个文件 · 累计 ${progress.foundFiles.toLocaleString()} 个 / ${formatBytes(progress.foundBytes)}`
                : "正在准备扫描清单…"}
            </Text>
            {progress?.currentPath && (
              <Text type="secondary" code style={{ fontSize: 11, wordBreak: "break-all" }}>
                {progress.currentPath}
              </Text>
            )}
          </Space>
        </div>
      )}
      {!report ? (
        <div
          style={{
            textAlign: "center",
            padding: "60px 0",
            color: "var(--ant-color-text-secondary)",
          }}
        >
          <ClearOutlined style={{ fontSize: 64, marginBottom: 16 }} />
          <div style={{ fontSize: 16, marginBottom: 8 }}>点击“扫描垃圾”开始分析</div>
          <div style={{ fontSize: 12 }}>扫描缓存、临时文件、回收站等；受保护目录会被自动跳过</div>
        </div>
      ) : (
        <>
          {report.cancelled && (
            <Alert
              type="warning"
              showIcon
              style={{ marginBottom: 16 }}
              title="本次扫描已被取消"
              description="下面的结果只包含取消前扫完的部分，不代表可清理空间的全貌；如需完整数据请重新扫描。"
            />
          )}
          <Row gutter={16} style={{ marginBottom: 16 }}>
            <Col span={8}>
              <Statistic
                title="可清理空间"
                value={formatBytes(report.totalSizeBytes)}
                styles={{ content: { color: "#52c41a", fontSize: 28 } }}
              />
            </Col>
            <Col span={8}>
              <Statistic
                title="可清理文件数"
                value={report.totalFiles.toLocaleString()}
                prefix={<FileSearchOutlined />}
              />
            </Col>
            <Col span={8}>
              <Statistic
                title="扫描耗时"
                value={`${report.scanTimeMs} ms`}
                styles={{ content: { color: "#1890ff" } }}
              />
            </Col>
          </Row>

          <Table<JunkCategory>
            rowKey="id"
            rowSelection={{
              selectedRowKeys: selectedIds,
              onChange: (keys) => onSelectedIdsChange(keys as string[]),
            }}
            columns={[
              {
                title: "类别",
                dataIndex: "name",
                render: (name: string, record) => (
                  <Space orientation="vertical" size={0}>
                    <strong>{name}</strong>
                    <Text type="secondary" style={{ fontSize: 12 }}>
                      {record.description}
                    </Text>
                  </Space>
                ),
              },
              {
                title: "风险等级",
                dataIndex: "riskLevel",
                width: 100,
                render: (level: string) => (
                  <Tag color={riskColor(level)}>{RISK_LABELS[level] ?? level}</Tag>
                ),
              },
              {
                title: "文件数",
                dataIndex: "fileCount",
                width: 120,
                render: (n: number) => n.toLocaleString(),
              },
              {
                title: "占用空间",
                dataIndex: "sizeBytes",
                width: 150,
                render: (size: number) => (
                  <span style={{ fontWeight: 600, color: "#52c41a" }}>{formatBytes(size)}</span>
                ),
              },
              {
                title: "路径",
                dataIndex: "paths",
                render: (paths: string[]) => (
                  <Space orientation="vertical" size={0}>
                    {paths.slice(0, 2).map((p, i) => (
                      <Text key={i} code style={{ fontSize: 11 }}>
                        {p}
                      </Text>
                    ))}
                    {paths.length > 2 && (
                      <Text type="secondary" style={{ fontSize: 11 }}>
                        …等 {paths.length} 个
                      </Text>
                    )}
                  </Space>
                ),
              },
            ]}
            dataSource={report.categories}
            pagination={false}
            size="middle"
          />
        </>
      )}
    </Card>
  );
}
