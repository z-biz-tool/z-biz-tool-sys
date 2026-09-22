import { Alert, Button, Card } from "antd";
import React from "react";

interface State {
  error: Error | null;
}

/**
 * 单个 Tab 渲染抛错不该白屏整窗（T4-08）：保留错误文案与重试入口。
 * 采集在后端独立运行，重挂子树即可继续收帧。
 */
export class ErrorBoundary extends React.Component<
  { children: React.ReactNode; label: string },
  State
> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: React.ErrorInfo) {
    console.error(`[${this.props.label}] 渲染失败`, error, info.componentStack);
  }

  render() {
    const { error } = this.state;
    if (!error) return this.props.children;
    return (
      <Card size="small" className="monitor-card">
        <Alert
          type="error"
          showIcon
          title={`${this.props.label}渲染失败`}
          description={error.message || "未知错误"}
          action={
            <Button size="small" onClick={() => this.setState({ error: null })}>
              重试
            </Button>
          }
        />
      </Card>
    );
  }
}
