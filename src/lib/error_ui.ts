// 后端 AppError.code → 界面行为的单点映射（T4-10）。
// 区分的是「该不该按故障报」：受保护路径被拒、进程已退出、平台不支持，都不是错误，
// 用 error 级别报会把防护生效说成系统坏了。
import { AppErrorCode, asAppError, describeError } from "../ipc_contract";

export type NoticeLevel = "error" | "warning" | "info";

export interface Notice {
  level: NoticeLevel;
  message: string;
  /** 失败说明界面缓存的数据已过期（如目标进程已退出），调用方应刷新对应列表 */
  staleData: boolean;
}

export function presentError(e: unknown, context: string): Notice {
  const error = asAppError(e);
  const reason = describeError(e);
  if (!error) {
    // 拿不到 code 说明 invoke 本身没通（WebView 无 IPC 通道、命令未注册），是真故障
    return { level: "error", message: `${context}：${reason}`, staleData: false };
  }

  switch (error.code) {
    case AppErrorCode.processNotFound:
    case AppErrorCode.notFound:
      return { level: "warning", message: `${context}：${reason}`, staleData: true };

    case AppErrorCode.pathDenied:
      return {
        level: "info",
        message: `${context}已拦下：${reason}（该位置受保护，应用不会尝试绕开）`,
        staleData: false,
      };

    case AppErrorCode.permissionDenied:
      return {
        level: "warning",
        message: `${context}被拒：${reason}（应用内不做提权，需提权的操作请在终端自行执行）`,
        staleData: false,
      };

    case AppErrorCode.unsupported:
      return { level: "info", message: `${context}：${reason}`, staleData: false };

    case AppErrorCode.invalidInput:
    case AppErrorCode.commandFailed:
    default:
      return { level: "error", message: `${context}：${reason}`, staleData: false };
  }
}
