use serde::Serialize;
use std::fmt;

/// 所有 command 的统一错误载荷：前端按 `code` 决定 UI 行为，`message` 可直接展示。
/// `detail` 仅在 debug 构建下携带，避免 release 泄漏本地路径等环境信息。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

macro_rules! error_constructor {
    ($name:ident, $code:expr) => {
        pub fn $name(message: impl Into<String>) -> Self {
            Self::new($code, message)
        }
    };
}

impl AppError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
            detail: None,
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        if cfg!(debug_assertions) {
            self.detail = Some(detail.into());
        }
        self
    }

    error_constructor!(permission_denied, "PERMISSION_DENIED");
    error_constructor!(not_found, "NOT_FOUND");
    error_constructor!(process_not_found, "PID_NOT_FOUND");
    error_constructor!(path_denied, "PATH_DENIED");
    error_constructor!(invalid_input, "INVALID_INPUT");
    error_constructor!(failed, "COMMAND_FAILED");
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        let kind = format!("{:?}", e.kind());
        AppError::failed(format!("命令执行失败: {}", e)).with_detail(kind)
    }
}

pub type CommandResult<T> = Result<T, AppError>;
