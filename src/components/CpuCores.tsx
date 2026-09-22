import { Card, Tooltip, Typography } from "antd";
import { formatPercent, usageColor } from "../lib/format";
import { gradientText, monitorCardStyle } from "../lib/ui";

const { Text } = Typography;

/** 每核心 CPU 竖条（T3-05）：仅渲染后端真实上报的 perCore */
export function CpuCores({ perCore }: { perCore: number[] }) {
  return (
    <Card
      size="small"
      className="monitor-card"
      style={{ ...monitorCardStyle }}
      title={
        <span style={{ ...gradientText, fontWeight: 600 }}>
          每核心 CPU（{perCore.length} 核）
        </span>
      }
    >
      <div style={{ display: "flex", flexWrap: "wrap", gap: 10 }}>
        {perCore.map((usage, core) => (
          <Tooltip key={core} title={`核心 ${core} · ${formatPercent(usage)}`}>
            <div style={{ width: 34, textAlign: "center" }}>
              <div
                style={{
                  height: 48,
                  display: "flex",
                  alignItems: "flex-end",
                  background: "rgba(128,128,128,0.14)",
                  borderRadius: 4,
                  overflow: "hidden",
                }}
              >
                <div
                  style={{
                    width: "100%",
                    height: `${Math.min(Math.max(usage, 0), 100)}%`,
                    background: usageColor(usage, 80),
                    transition: "height 0.3s",
                  }}
                />
              </div>
              <Text type="secondary" style={{ fontSize: 11 }}>
                #{core}
              </Text>
            </div>
          </Tooltip>
        ))}
      </div>
    </Card>
  );
}
