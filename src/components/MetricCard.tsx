import { Card, Progress, Typography } from "antd";
import { usageColor } from "../lib/format";
import { gradientText, monitorCardStyle } from "../lib/ui";

const { Text } = Typography;

export function MetricCard({
  icon,
  title,
  value,
  unit,
  percent,
  footer,
}: {
  icon: React.ReactNode;
  title: string;
  value: string;
  unit?: string;
  percent?: number;
  footer: React.ReactNode;
}) {
  return (
    <Card size="small" className="monitor-card" style={monitorCardStyle}>
      <div className="monitor-card-title" style={gradientText}>
        {icon} {title}
      </div>
      <div className="monitor-card-value">
        {value}
        {unit ? <span className="monitor-card-unit">{unit}</span> : null}
      </div>
      {percent === undefined ? (
        <div style={{ height: "8px" }} />
      ) : (
        <Progress
          percent={Math.min(percent, 100)}
          strokeColor={usageColor(percent, 80)}
          showInfo={false}
        />
      )}
      <Text type="secondary" style={{ fontSize: 12 }}>
        {footer}
      </Text>
    </Card>
  );
}
