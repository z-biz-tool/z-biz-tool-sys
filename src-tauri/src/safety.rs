use crate::error::{AppError, CommandResult};
use serde::Serialize;
use std::time::Duration;

/// 这些进程一旦被结束会直接导致会话掉线或系统不可用，因此后端硬拒。
#[cfg(windows)]
const PROTECTED_NAMES: &[&str] = &[
    "csrss.exe",
    "wininit.exe",
    "winlogon.exe",
    "services.exe",
    "lsass.exe",
    "svchost.exe",
    "smss.exe",
    "dwm.exe",
    "explorer.exe",
];

#[cfg(not(windows))]
const PROTECTED_NAMES: &[&str] = &[
    "launchd",
    "kernel_task",
    "loginwindow",
    "WindowServer",
    "System Events",
    "UserEventAgent",
    "configd",
    "diskarbitrationd",
    "fseventsd",
    "notifyd",
    "opendirectoryd",
    "distnoted",
    "runningboardd",
    "audiomxd",
    "mediaserverd",
    "Dock",
    "Finder",
    "ControlCenter",
    "WindowManager",
    "init",
    "systemd",
    "kthreadd",
    "systemd-udevd",
    "sshd",
    "gdm-session-worker",
];

/// 结束这些进程会立刻中断用户图形会话，要求逐字输入进程名确认。
const CRITICAL_NAMES: &[&str] = &[
    "Finder", "Dock", "ControlCenter", "WindowServer", "explorer.exe", "dwm.exe",
];

/// PID 不超过该值视为系统进程，禁止结束。
const SYSTEM_PID_CEILING: u32 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum KillRiskLevel {
    Blocked,
    Critical,
    Standard,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KillValidation {
    pub pid: u32,
    pub exists: bool,
    pub allowed: bool,
    pub risk_level: KillRiskLevel,
    pub process_name: String,
    pub user_name: Option<String>,
    pub memory_bytes: u64,
    pub is_self: bool,
    pub denied_reason: Option<String>,
}

fn is_protected(name: &str) -> bool {
    PROTECTED_NAMES
        .iter()
        .any(|p| p.eq_ignore_ascii_case(name) || name.eq_ignore_ascii_case(p))
}

fn is_critical(name: &str) -> bool {
    CRITICAL_NAMES.iter().any(|c| c.eq_ignore_ascii_case(name))
}

/// 当前系统用户登录名，用于确认对话框展示归属。
pub fn current_user_name() -> Option<String> {
    ["USER", "USERNAME", "LOGNAME"]
        .iter()
        .find_map(|k| std::env::var(k).ok())
        .filter(|v| !v.is_empty())
}

fn process_exists(pid: u32) -> Result<bool, String> {
    #[cfg(unix)]
    {
        let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
        if rc == 0 {
            return Ok(true);
        }
        match std::io::Error::last_os_error().raw_os_error() {
            Some(libc::ESRCH) => Ok(false),
            // 进程存在但不属于当前用户，OS 会直接拒绝信号。
            Some(libc::EPERM) => Err("PERMISSION_DENIED".to_string()),
            _ => Ok(false),
        }
    }
    #[cfg(not(unix))]
    {
        Ok(crate::monitor::lookup_process(pid).is_some())
    }
}

/// 校验能否结束 `pid`。前端点击「结束」时先调用本函数，展示真实进程名再确认。
pub fn validate_kill(pid: u32) -> CommandResult<KillValidation> {
    let denied = |reason: String, level: KillRiskLevel| KillValidation {
        pid,
        exists: true,
        allowed: false,
        risk_level: level,
        process_name: String::new(),
        user_name: None,
        memory_bytes: 0,
        is_self: false,
        denied_reason: Some(reason),
    };

    if pid == 0 {
        return Err(AppError::invalid_input("PID 不能为 0"));
    }

    if pid == std::process::id() {
        return Ok(KillValidation {
            pid,
            exists: true,
            allowed: false,
            risk_level: KillRiskLevel::Blocked,
            process_name: "z-biz-tool-sys".to_string(),
            user_name: current_user_name(),
            memory_bytes: 0,
            is_self: true,
            denied_reason: Some("不能结束本应用自身进程".to_string()),
        });
    }

    match process_exists(pid) {
        Ok(false) => return Err(AppError::process_not_found(format!("PID {pid} 不存在"))),
        Err(_) => {
            return Ok(denied(
                "该进程属于其他用户，当前权限无法结束".to_string(),
                KillRiskLevel::Blocked,
            ))
        }
        Ok(true) => {}
    }

    let info = crate::monitor::lookup_process(pid)
        .ok_or_else(|| AppError::process_not_found(format!("PID {pid} 不存在")))?;

    if pid <= SYSTEM_PID_CEILING {
        return Ok(KillValidation {
            process_name: info.name.clone(),
            ..denied(
                format!(
                    "PID {} 属于系统进程（PID ≤ {}），已禁止结束",
                    pid, SYSTEM_PID_CEILING
                ),
                KillRiskLevel::Blocked,
            )
        });
    }

    if is_protected(&info.name) {
        return Ok(KillValidation {
            process_name: info.name.clone(),
            ..denied(
                format!("{} 是关键系统进程，已禁止结束", info.name),
                KillRiskLevel::Blocked,
            )
        });
    }

    let level = if is_critical(&info.name) {
        KillRiskLevel::Critical
    } else {
        KillRiskLevel::Standard
    };

    Ok(KillValidation {
        pid,
        exists: true,
        allowed: true,
        risk_level: level,
        process_name: info.name.clone(),
        user_name: current_user_name(),
        memory_bytes: info.memory_bytes,
        is_self: false,
        denied_reason: None,
    })
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KillOutcome {
    pub pid: u32,
    pub process_name: String,
    /// true 表示进程在 SIGTERM 阶段即退出，未升级到 SIGKILL。
    pub terminated_gracefully: bool,
    pub elapsed_ms: u64,
}

fn signal_term(pid: u32) -> Result<(), AppError> {
    #[cfg(unix)]
    {
        let rc = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
        if rc == 0 {
            Ok(())
        } else {
            Err(AppError::failed(format!(
                "发送 SIGTERM 失败: {}",
                std::io::Error::last_os_error()
            )))
        }
    }
    #[cfg(windows)]
    {
        let out = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string()])
            .output()?;
        if out.status.success() {
            Ok(())
        } else {
            Err(AppError::failed(
                String::from_utf8_lossy(&out.stderr).trim().to_string(),
            ))
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        Err(AppError::unsupported("当前平台不支持结束进程"))
    }
}

fn signal_kill(pid: u32) -> Result<(), AppError> {
    #[cfg(unix)]
    {
        let rc = unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
        if rc == 0 {
            Ok(())
        } else {
            Err(AppError::failed(format!(
                "发送 SIGKILL 失败: {}",
                std::io::Error::last_os_error()
            )))
        }
    }
    #[cfg(windows)]
    {
        let out = std::process::Command::new("taskkill")
            .args(["/F", "/PID", &pid.to_string()])
            .output()?;
        if out.status.success() {
            Ok(())
        } else {
            Err(AppError::failed(
                String::from_utf8_lossy(&out.stderr).trim().to_string(),
            ))
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        Err(AppError::unsupported("当前平台不支持结束进程"))
    }
}

fn still_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        crate::monitor::lookup_process(pid).is_some()
    }
}

/// 先 SIGTERM 给进程清理机会，超时后才 SIGKILL。
pub async fn terminate(pid: u32, grace: Duration) -> CommandResult<KillOutcome> {
    let validation = validate_kill(pid)?;
    if !validation.allowed {
        return Err(AppError::permission_denied(
            validation
                .denied_reason
                .unwrap_or_else(|| "该进程不允许结束".to_string()),
        ));
    }

    let started = std::time::Instant::now();
    signal_term(pid)?;

    let poll = Duration::from_millis(50);
    let deadline = started + grace;
    while std::time::Instant::now() < deadline {
        if !still_alive(pid) {
            return Ok(KillOutcome {
                pid,
                process_name: validation.process_name,
                terminated_gracefully: true,
                elapsed_ms: started.elapsed().as_millis() as u64,
            });
        }
        tokio::time::sleep(poll).await;
    }

    signal_kill(pid)?;

    let hard_deadline = std::time::Instant::now() + Duration::from_millis(1500);
    while std::time::Instant::now() < hard_deadline {
        if !still_alive(pid) {
            return Ok(KillOutcome {
                pid,
                process_name: validation.process_name,
                terminated_gracefully: false,
                elapsed_ms: started.elapsed().as_millis() as u64,
            });
        }
        tokio::time::sleep(poll).await;
    }

    Err(AppError::failed(format!(
        "PID {} ({}) 在 {}ms 内未退出",
        pid,
        validation.process_name,
        started.elapsed().as_millis()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_self_and_zero_pid() {
        let me = std::process::id();
        let v = validate_kill(me).unwrap();
        assert!(!v.allowed);
        assert_eq!(v.risk_level, KillRiskLevel::Blocked);
        assert!(v.is_self);
        assert!(matches!(
            validate_kill(0),
            Err(AppError { ref code, .. }) if code == "INVALID_INPUT"
        ));
    }

    #[test]
    fn rejects_missing_pid_with_typed_error() {
        let err = validate_kill(u32::MAX - 1).unwrap_err();
        assert_eq!(err.code, "PID_NOT_FOUND");
    }

    #[test]
    fn low_pids_are_blocked() {
        for pid in 1..=SYSTEM_PID_CEILING {
            if let Ok(v) = validate_kill(pid) {
                assert!(!v.allowed, "pid {} must not be killable", pid);
                assert_eq!(v.risk_level, KillRiskLevel::Blocked);
            }
        }
    }

    #[test]
    fn protection_lists_are_lowercase_free_of_duplicates() {
        assert!(is_protected("launchd") || cfg!(windows));
        assert!(is_protected("LAUNCHD"));
        assert!(is_critical("Finder"));
        assert!(!is_critical("Safari"));
        assert_eq!(PROTECTED_NAMES.len(), std::collections::HashSet::<_>::from_iter(PROTECTED_NAMES.iter().map(|s| s.to_lowercase())).len(), "protected list has duplicates");
    }

    #[tokio::test]
    async fn terminate_refuses_blocked_targets() {
        let err = terminate(std::process::id(), Duration::from_millis(50))
            .await
            .unwrap_err();
        assert_eq!(err.code, "PERMISSION_DENIED");
    }
}
