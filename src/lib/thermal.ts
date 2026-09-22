// 温度/风扇读数的展示口径（T5-08）。
// 判级不在这里：`severity` 是后端按硬件自己上报的上限算出来的（`src-tauri/src/platform/thermal.rs`
// 里的 `WARN_HEADROOM_C` 是全项目唯一的温度阈值），前端只做上色与文案 —— 与告警那套"判定在后端"一致。
import type { SensorSeverity, ThermalSensor } from "../ipc_contract";

/** `unknown` 刻意不上色：那块传感器没上报上限，说它"正常"就是编数据。 */
export const SEVERITY_COLOR: Record<SensorSeverity, string | undefined> = {
  ok: "#52c41a",
  warning: "#faad14",
  critical: "#ff4d4f",
  unknown: undefined,
};

const SEVERITY_TEXT: Record<SensorSeverity, string> = {
  ok: "正常",
  warning: "偏高",
  critical: "越过硬件上限",
  unknown: "未上报上限",
};

/**
 * 状态列的文案。风扇的 `warning` 只能是"转速为 0"，写成"偏高"是反的
 * （0 RPM 不是"偏高"，是停转），所以这一列按种类分岔；判级仍然只在后端算。
 */
export function severityText(sensor: ThermalSensor): string {
  if (sensor.kind === "fan" && sensor.severity === "warning") return "停转";
  return SEVERITY_TEXT[sensor.severity];
}

/** 温度留一位小数（sysfs 本来就是毫摄氏度），风扇按整数 RPM。 */
export function formatSensorValue(sensor: ThermalSensor): string {
  return sensor.kind === "temperature"
    ? `${sensor.value.toFixed(1)} °C`
    : `${Math.round(sensor.value)} RPM`;
}

/** 硬件上限列：没有就是 `—`，不补一个"常见的 90 °C"。 */
export function formatSensorLimit(sensor: ThermalSensor): string {
  return sensor.critical == null ? "—" : `${sensor.critical.toFixed(0)} °C`;
}
