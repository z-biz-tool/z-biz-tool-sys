import { useState } from "react";
import { Alert, Button, Divider, Modal, Space, Tag, Typography } from "antd";
import { ExportOutlined, ImportOutlined } from "@ant-design/icons";
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
import dayjs from "dayjs";
import { presentError } from "../lib/error_ui";
import {
  defaultPrefsFileName,
  interpretImportedPrefs,
  rawValueText,
  type PrefsInterpretation,
} from "../lib/prefs_file";
import {
  Commands,
  type PrefsExportOutcome,
  type PrefsImportOutcome,
  type PrefsSnapshot,
} from "../ipc_contract";

const { Text, Paragraph } = Typography;

const FILTERS = [{ name: "偏好文件", extensions: ["json"] }];

export type PrefsNoticeLevel = "success" | "warning" | "error" | "info";

interface Props {
  open: boolean;
  onClose: () => void;
  /** 当前偏好快照：导出的内容、导入的比较基准都是它 */
  snapshot: PrefsSnapshot;
  /** 真实页签 key 列表；白名单仍由 App 的 tabItems 提供，这里不另写一份 */
  tabs: string[];
  onApply: (next: PrefsSnapshot) => void;
  onNotice: (level: PrefsNoticeLevel, text: string) => void;
}

/** 只取文件名：完整路径不进界面，避免截图/录屏带出用户目录。 */
const baseName = (path: string) => path.split(/[\\/]/).pop() ?? path;

const OUTCOME_TAG: Record<
  PrefsInterpretation["entries"][number]["outcome"],
  { color: string; text: string }
> = {
  adopted: { color: "green", text: "采纳" },
  clamped: { color: "orange", text: "已夹取" },
  absent: { color: "default", text: "文件没有" },
  invalid: { color: "red", text: "值不可用" },
};

/** 保存框/选择框自己报错时不必按"命令失败"分级，统一这一档 */
const dialogNotice = (e: unknown, context: string) => {
  const notice = presentError(e, context);
  return (notice.level === "error" ? "error" : "info") as PrefsNoticeLevel;
};

/**
 * 偏好导入 / 导出（T5-11）。
 *
 * 两步式：导入只把文件读进来给出逐项预览，用户再点"应用"才动界面 ——
 * 一份来历不明的 JSON 不该一选就把主题、阈值、页签全换掉。
 * 后端只认"这是不是本应用这份格式"，值合不合法全在 `lib/prefs_file.ts` 那套白名单里判，
 * 所以脏值只会回落默认并如实标出，不会让界面塌掉。
 */
export function PrefsTransferModal({ open, onClose, snapshot, tabs, onApply, onNotice }: Props) {
  const [busy, setBusy] = useState(false);
  const [pending, setPending] = useState<{
    file: PrefsImportOutcome;
    parsed: PrefsInterpretation;
  } | null>(null);

  const doExport = async () => {
    let picked: string | null = null;
    try {
      picked = await saveDialog({
        title: "导出偏好为 JSON",
        defaultPath: defaultPrefsFileName(),
        filters: FILTERS,
      });
    } catch (e) {
      onNotice(dialogNotice(e, "打开保存框"), presentError(e, "打开保存框").message);
      return;
    }
    if (typeof picked !== "string") return; // 用户取消：什么都不做，也不报"失败"
    setBusy(true);
    try {
      const out = await invoke<PrefsExportOutcome>(Commands.exportPrefsFile, {
        path: picked,
        prefs: snapshot,
      });
      onNotice(
        "success",
        `已导出 ${out.keys} 项偏好（${out.bytes} B，格式 v${out.schemaVersion}）到 ${baseName(out.path)}`
      );
    } catch (e) {
      const notice = presentError(e, "导出偏好");
      onNotice(notice.level === "info" ? "info" : "error", notice.message);
    } finally {
      setBusy(false);
    }
  };

  const doImport = async () => {
    let picked: string | null = null;
    try {
      picked = await openDialog({
        title: "选择要导入的偏好文件",
        multiple: false,
        directory: false,
        filters: FILTERS,
      });
    } catch (e) {
      onNotice(dialogNotice(e, "打开选择框"), presentError(e, "打开选择框").message);
      return;
    }
    if (typeof picked !== "string") return;
    setBusy(true);
    try {
      const file = await invoke<PrefsImportOutcome>(Commands.importPrefsFile, { path: picked });
      setPending({ file, parsed: interpretImportedPrefs(file.prefs, snapshot, tabs) });
    } catch (e) {
      const notice = presentError(e, "导入偏好");
      onNotice(notice.level === "info" ? "info" : "error", notice.message);
    } finally {
      setBusy(false);
    }
  };

  const apply = () => {
    if (!pending) return;
    const { parsed, file } = pending;
    onApply(parsed.next);
    setPending(null);
    onNotice(
      parsed.changed === 0 ? "info" : "success",
      parsed.changed === 0
        ? `${baseName(file.path)} 的生效值与当前配置相同，没有改动`
        : `已应用 ${parsed.changed} 项偏好（采纳 ${parsed.adopted}、夹取 ${parsed.clamped}、保持现状 ${parsed.kept}）`
    );
  };

  return (
    <Modal
      open={open}
      title="偏好导入 / 导出"
      onCancel={() => {
        setPending(null);
        onClose();
      }}
      footer={null}
      width={620}
    >
      <Paragraph style={{ marginBottom: 12 }}>
        文件里只有这 5 项设置：深色主题、采集间隔、趋势窗口、默认页签、告警阈值。
        不含路径、进程或主机信息。导入会先给出逐项预览，点了"应用"才生效。
      </Paragraph>
      <Space wrap>
        <Button icon={<ExportOutlined />} loading={busy} onClick={doExport}>
          导出为 JSON…
        </Button>
        <Button icon={<ImportOutlined />} loading={busy} onClick={doImport}>
          从 JSON 导入…
        </Button>
      </Space>

      {pending && (
        <>
          <Divider style={{ margin: "14px 0 10px" }} />
          <Space orientation="vertical" size={4} style={{ width: "100%" }}>
            <Text strong>{baseName(pending.file.path)}</Text>
            <Text type="secondary" style={{ fontSize: 12 }}>
              {pending.file.bytes} B · 格式 v{pending.file.schemaVersion} · 文件自带{" "}
              {pending.file.fileKeys} 项 ·{" "}
              {pending.file.exportedAtMs === null
                ? "导出时间未记录"
                : `导出于 ${dayjs(pending.file.exportedAtMs).format("YYYY-MM-DD HH:mm:ss")}`}
            </Text>

            {pending.parsed.entries.map((entry) => {
              const kept = entry.outcome === "absent" || entry.outcome === "invalid";
              return (
                <Space key={entry.key} align="start" wrap size={6}>
                  <Tag color={OUTCOME_TAG[entry.outcome].color}>{entry.label}</Tag>
                  <Tag>{OUTCOME_TAG[entry.outcome].text}</Tag>
                  <Text style={{ fontSize: 12 }}>
                    {kept ? `保持现状：${entry.text}` : entry.text}
                    {entry.differs && " （与当前不同）"}
                  </Text>
                  {/* 只要文件里带了这一项，就把原值显出来：夹取和判废都得让用户能回指"我写的到底是什么" */}
                  {entry.outcome !== "adopted" && entry.outcome !== "absent" && (
                    <Text type="secondary" style={{ fontSize: 12 }}>
                      文件里是 {rawValueText(entry.raw)}
                    </Text>
                  )}
                </Space>
              );
            })}

            {pending.parsed.unknownKeys.length > 0 && (
              <Alert
                type="warning"
                showIcon
                title={`文件里有 ${pending.parsed.unknownKeys.length} 项本版本不认，已忽略`}
                description={`${pending.parsed.unknownKeys
                  .slice(0, 8)
                  .join("、")}${
                  pending.parsed.unknownKeys.length > 8
                    ? ` 等 ${pending.parsed.unknownKeys.length} 项`
                    : ""
                } —— 不认识的键不会被写到任何地方。`}
              />
            )}
            {pending.parsed.changed === 0 && (
              <Alert
                type="info"
                showIcon
                title="这份文件的生效值与当前配置完全相同，应用它不会有任何改动。"
              />
            )}

            <Space style={{ marginTop: 6 }}>
              <Button type="primary" onClick={apply}>
                应用 {pending.parsed.changed} 项
              </Button>
              <Button onClick={() => setPending(null)}>放弃</Button>
            </Space>
          </Space>
        </>
      )}
    </Modal>
  );
}
