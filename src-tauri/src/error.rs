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
    // safety.rs 的非 unix/非 windows 分支引用它；本机看不到调用点，但删掉它就是让那些目标编不过。
    // 不用 `error_constructor!` 是因为属性加在宏调用上会被忽略，只能写成显式 fn 才能放行 dead_code。
    #[allow(dead_code)]
    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::new("UNSUPPORTED", message)
    }
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

/// code 是前端 `lib/error_ui.ts` 决定提示级别与是否刷新数据的唯一依据，
/// 因此两侧集合必须严格相等：后端加了构造器而前端没分支，或前端留了分支而后端不产，都要在这里失败。
#[cfg(test)]
mod tests {
    use super::AppError;
    use std::fs;

    #[test]
    fn every_constructor_emits_its_registered_code() {
        let cases: &[(&str, AppError)] = &[
            ("PERMISSION_DENIED", AppError::permission_denied("x")),
            ("NOT_FOUND", AppError::not_found("x")),
            ("PID_NOT_FOUND", AppError::process_not_found("x")),
            ("PATH_DENIED", AppError::path_denied("x")),
            ("INVALID_INPUT", AppError::invalid_input("x")),
            ("COMMAND_FAILED", AppError::failed("x")),
            ("UNSUPPORTED", AppError::unsupported("x")),
        ];
        for (expected, error) in cases {
            assert_eq!(error.code, *expected, "构造器给出的 code 与登记值不符");
        }
    }

    #[test]
    fn error_codes_match_the_frontend_ipc_contract() {
        let ts = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/ipc_contract.ts");
        let contract = fs::read_to_string(&ts).expect("前端 IPC 契约文件必须存在");

        let start = contract
            .find("export const AppErrorCode = {")
            .expect("前端未登记 AppErrorCode 映射");
        let block = &contract[start..contract[start..].find("} as const;").expect("AppErrorCode 缺收尾") + start];

        let mut front: Vec<String> = block
            .match_indices('"')
            .map(|(index, _)| index)
            .collect::<Vec<_>>()
            .chunks(2)
            .map(|pair| block[pair[0] + 1..pair[1]].to_string())
            .collect();
        front.sort();

        let mut back: Vec<String> = vec![
            AppError::permission_denied("x").code,
            AppError::not_found("x").code,
            AppError::process_not_found("x").code,
            AppError::path_denied("x").code,
            AppError::invalid_input("x").code,
            AppError::failed("x").code,
            AppError::unsupported("x").code,
        ];
        back.sort();

        assert_eq!(
            front, back,
            "前端 AppErrorCode 与后端 AppError 构造器的 code 集合不一致"
        );
    }

    #[test]
    fn detail_is_only_serialized_in_debug_builds() {
        let with_detail = AppError::failed("x").with_detail("/Users/someone/private");
        let json = serde_json::to_string(&with_detail).unwrap();
        if cfg!(debug_assertions) {
            assert!(json.contains("\"detail\""), "{json}");
            // 前端契约里字段名是 camelCase
            assert!(json.contains("\"code\"") && json.contains("\"message\""), "{json}");
        } else {
            assert!(!json.contains("detail"), "release 不得带出 detail: {json}");
        }

        // 无 detail 时整个字段缺席，而不是给前端一个 null 去分辨
        let plain = serde_json::to_string(&AppError::failed("x")).unwrap();
        assert!(!plain.contains("detail"), "{plain}");
    }
}
