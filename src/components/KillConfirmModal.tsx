import { Input, Modal, Space, Typography } from "antd";
import type { KillValidation } from "../ipc_contract";
import { formatBytes } from "../lib/format";

const { Text } = Typography;

interface Props {
  target: KillValidation | null;
  confirmText: string;
  killing: boolean;
  onConfirmTextChange: (value: string) => void;
  onCancel: () => void;
  onConfirm: () => void;
  afterClose: () => void;
}

export function KillConfirmModal({
  target,
  confirmText,
  killing,
  onConfirmTextChange,
  onCancel,
  onConfirm,
  afterClose,
}: Props) {
  const critical = target?.riskLevel === "critical";
  return (
    <Modal
      open={!!target}
      title={`结束进程 ${target?.processName ?? ""}`}
      onCancel={onCancel}
      onOk={onConfirm}
      okText="确认结束"
      okButtonProps={{
        danger: true,
        loading: killing,
        disabled: critical && confirmText !== target?.processName,
      }}
      afterClose={afterClose}
    >
      {target ? (
        <Space orientation="vertical" size="small" style={{ width: "100%" }}>
          <Text>
            PID <Text code>{target.pid}</Text> · 内存 {formatBytes(target.memoryBytes, 1)}
            {target.userName ? ` · 属主 ${target.userName}` : ""}
          </Text>
          <Text type="danger">
            {critical
              ? "该进程中断会立刻影响图形会话，请输入进程名以确认。"
              : "结束该进程可能丢失未保存的数据。"}
          </Text>
          {critical && (
            <Input
              placeholder={`输入 ${target.processName} 以确认`}
              value={confirmText}
              onChange={(e) => onConfirmTextChange(e.target.value)}
            />
          )}
        </Space>
      ) : null}
    </Modal>
  );
}
