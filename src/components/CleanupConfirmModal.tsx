import { Alert, Modal, Space, Tag, Typography } from "antd";
import { useMemo } from "react";
import type { JunkReport } from "../ipc_contract";
import { formatBytes } from "../lib/format";

const { Text } = Typography;

interface Props {
  open: boolean;
  report: JunkReport | null;
  selectedIds: string[];
  cleaning: boolean;
  onCancel: () => void;
  onConfirm: () => void;
}

/** 清理是不可逆操作（T3-10）：确认框必须列出真实落点目录，而不是只问一句"确定吗" */
export function CleanupConfirmModal({
  open,
  report,
  selectedIds,
  cleaning,
  onCancel,
  onConfirm,
}: Props) {
  const selected = useMemo(
    () => (report?.categories ?? []).filter((c) => selectedIds.includes(c.id)),
    [report, selectedIds]
  );
  const bytes = selected.reduce((sum, c) => sum + c.sizeBytes, 0);
  const files = selected.reduce((sum, c) => sum + c.fileCount, 0);
  const paths = useMemo(
    () => Array.from(new Set(selected.flatMap((c) => c.paths))).sort(),
    [selected]
  );

  return (
    <Modal
      open={open}
      title="确认清理选中项"
      width={640}
      okText="永久删除"
      okButtonProps={{ danger: true, loading: cleaning }}
      onCancel={onCancel}
      onOk={onConfirm}
    >
      <Space orientation="vertical" size="small" style={{ width: "100%" }}>
        <Alert
          type="error"
          showIcon
          title="删除后不可恢复"
          description="后端逐个调用 remove_file 直接抹除文件，不经过回收站；请核对下方目录后再确认。"
        />
        <Text>
          共 <Text strong>{selected.length}</Text> 类 ·{" "}
          <Text strong>{files.toLocaleString()}</Text> 个文件 · 预计释放{" "}
          <Text strong>{formatBytes(bytes)}</Text>
        </Text>
        <div>
          {selected.map((c) => (
            <div key={c.id} style={{ marginBottom: 4 }}>
              <Space size={6}>
                <Tag color={c.riskLevel === "safe" ? "green" : "orange"}>
                  {c.riskLevel === "safe" ? "安全" : "谨慎"}
                </Tag>
                <Text>{c.name}</Text>
                <Text type="secondary">{formatBytes(c.sizeBytes, 1)}</Text>
              </Space>
            </div>
          ))}
        </div>
        <Text type="secondary" style={{ fontSize: 12 }}>
          待清理目录（{paths.length}）
        </Text>
        <div
          style={{
            maxHeight: 180,
            overflow: "auto",
            padding: 8,
            borderRadius: 6,
            background: "rgba(127, 127, 127, 0.12)",
          }}
        >
          {paths.map((p) => (
            <div key={p}>
              <Text code style={{ fontSize: 12 }}>
                {p}
              </Text>
            </div>
          ))}
        </div>
      </Space>
    </Modal>
  );
}
