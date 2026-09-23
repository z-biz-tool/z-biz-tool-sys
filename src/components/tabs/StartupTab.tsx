import { Alert, Button, Card, Input, Modal, Space, Table, Tag, Tooltip, Typography } from "antd";
import { ReloadOutlined, RocketOutlined } from "@ant-design/icons";
import { useState } from "react";
import type { StartupAction, StartupItem } from "../../ipc_contract";

const { Text } = Typography;

interface Props {
  items: StartupItem[];
  loading: boolean;
  /** 一次操作正在进行中（同一页同时只允许一个，避免两个 rename 抢同一个落点） */
  busy: boolean;
  onReload: () => void;
  onDisable: (item: StartupItem) => void;
  onRemove: (item: StartupItem) => void;
  onRestore: (item: StartupItem) => void;
}

const ACTION_COPY: Record<
  StartupAction,
  { title: string; ok: string; note: string; danger: boolean }
> = {
  disable: {
    title: "禁用启动项",
    ok: "确认禁用",
    note: "原件整份移进本应用的备份目录，不删任何内容；下次登录不再加载。这一项仍会列在下面（标成未启用），随时可以恢复。",
    danger: false,
  },
  remove: {
    title: "删除启动项",
    ok: "确认删除",
    note: "原件同样移进备份目录，但从这个列表里消失。本应用不做物理删除 —— 要彻底清掉，请按确认框里给出的备份路径自己去删。",
    danger: true,
  },
  restore: {
    title: "恢复启动项",
    ok: "确认恢复",
    note: "备份里的原件移回启动目录，下次登录重新加载。启动目录里已经有同名文件时后端会拒绝，不会盖掉那一份。",
    danger: false,
  },
};

/** 不支持的来源为什么动不了：这三条通路都要提权或系统授权，后端直接回 UNSUPPORTED */
const NOT_OPERABLE_HINT =
  "这一项的来源需要提权或系统自动化授权（Login Items 走 System Events、注册表 Run 键要写注册表、systemd 单元要调 systemctl），本应用不会尝试改写它";

export function StartupTab({ items, loading, busy, onReload, onDisable, onRemove, onRestore }: Props) {
  const [pending, setPending] = useState<{ item: StartupItem; action: StartupAction } | null>(null);
  const [confirmText, setConfirmText] = useState("");
  const copy = pending ? ACTION_COPY[pending.action] : null;
  // 前后端各有一道确认：这里挡住"手滑点"，后端挡住"没经过确认框就被调用"
  const confirmed = !!pending && confirmText === pending.item.name;

  const close = () => {
    setPending(null);
    setConfirmText("");
  };

  return (
    <Card
      title={
        <Space>
          <RocketOutlined style={{ color: "#722ed1" }} />
          <span>开机启动项</span>
        </Space>
      }
      extra={
        <Button icon={<ReloadOutlined />} onClick={onReload} loading={loading}>
          刷新
        </Button>
      }
    >
      <Alert
        type="info"
        showIcon
        style={{ marginBottom: 12 }}
        title="这里的禁用与删除都只是一次移动，不删内容"
        description={
          <Space orientation="vertical" size={2}>
            <Text>
              能操作的只有放在启动目录里的文件（macOS 的 ~/Library/LaunchAgents、Linux 的
              ~/.config/autostart）。禁用＝移进本应用备份目录并仍列在下面；删除＝同样移进备份，但不再列出来。
            </Text>
            <Text type="secondary">
              状态列说的是"文件在不在启动目录里"，我们没有去猜 plist 里的 Disabled 键。
            </Text>
          </Space>
        }
      />
      <Table<StartupItem>
        rowKey="id"
        columns={[
          { title: "名称", dataIndex: "name", render: (n: string) => <strong>{n}</strong> },
          {
            title: "来源",
            dataIndex: "source",
            width: 170,
            render: (source: string, item) => (
              <Tag color={item.operable ? "blue" : "default"}>{source}</Tag>
            ),
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
            render: (e: boolean) => (e ? <Tag color="success">在启动目录</Tag> : <Tag>已禁用（备份中）</Tag>),
          },
          {
            title: "操作",
            key: "action",
            width: 160,
            render: (_, item) => {
              if (!item.operable) {
                return (
                  <Tooltip title={NOT_OPERABLE_HINT}>
                    <Tag>不支持在此操作</Tag>
                  </Tooltip>
                );
              }
              if (!item.enabled) {
                return (
                  <Button size="small" disabled={busy} onClick={() => setPending({ item, action: "restore" })}>
                    恢复
                  </Button>
                );
              }
              return (
                <Space size="small">
                  <Button size="small" disabled={busy} onClick={() => setPending({ item, action: "disable" })}>
                    禁用
                  </Button>
                  <Button size="small" danger disabled={busy} onClick={() => setPending({ item, action: "remove" })}>
                    删除
                  </Button>
                </Space>
              );
            },
          },
        ]}
        dataSource={items}
        loading={loading}
        pagination={false}
        locale={{ emptyText: "点击“刷新”读取系统启动项" }}
      />

      <Modal
        open={!!pending}
        title={copy ? `${copy.title} ${pending?.item.name ?? ""}` : ""}
        okText={copy?.ok}
        okButtonProps={{ danger: copy?.danger, loading: busy, disabled: !confirmed }}
        cancelText="取消"
        onCancel={close}
        afterClose={() => setConfirmText("")}
        onOk={() => {
          if (!pending || !confirmed) return;
          const { item, action } = pending;
          close();
          if (action === "disable") onDisable(item);
          else if (action === "remove") onRemove(item);
          else onRestore(item);
        }}
      >
        {pending && copy ? (
          <Space orientation="vertical" size="small" style={{ width: "100%" }}>
            <Text>
              <Text strong>{pending.item.name}</Text> · {pending.item.source}
            </Text>
            <Text code style={{ fontSize: 12 }}>
              {pending.item.command}
            </Text>
            <Alert type={copy.danger ? "warning" : "info"} showIcon title={copy.note} />
            <Text type="secondary">后端只认这一项的 id 与名称，界面提交不了路径。</Text>
            <Input
              placeholder={`输入 ${pending.item.name} 以确认`}
              value={confirmText}
              onChange={(e) => setConfirmText(e.target.value)}
              onPressEnter={() => {
                if (!confirmed) return;
                const { item, action } = pending;
                close();
                if (action === "disable") onDisable(item);
                else if (action === "remove") onRemove(item);
                else onRestore(item);
              }}
            />
          </Space>
        ) : null}
      </Modal>
    </Card>
  );
}
