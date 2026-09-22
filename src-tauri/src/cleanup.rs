// 系统清理模块：垃圾文件扫描、清理、大文件查找等
use crate::error::{AppError, CommandResult};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use walkdir::WalkDir;

/// 清理与扫描的落地根目录：只有家目录与临时目录之下才允许删除。
fn allowed_roots() -> Vec<PathBuf> {
    let mut roots = vec![std::env::temp_dir()];
    if let Some(home) = dirs::home_dir() {
        roots.push(home);
    }
    #[cfg(target_os = "macos")]
    {
        // $TMPDIR 常是 /var/folders/xx/.../T/，与 temp_dir() 一致，这里补上 /private 真实路径。
        if let Some(tmp) = std::env::var_os("TMPDIR") {
            roots.push(PathBuf::from(tmp));
        }
    }
    roots
}

/// 任何删除入口都要先过黑名单：系统目录、凭据目录不允许被扫描或清空。
const DENIED_PREFIXES: &[&str] = &[
    "/System",
    "/usr",
    "/bin",
    "/sbin",
    "/etc",
    "/var/db",
    "/Library",
    "/Applications",
    ".ssh",
    ".gnupg",
    "C:\\Windows",
    "C:\\Program Files",
];

fn is_denied(path: &Path) -> bool {
    let text = path.to_string_lossy();
    DENIED_PREFIXES.iter().any(|prefix| {
        if prefix.starts_with('/') || prefix.starts_with("C:") {
            text.starts_with(prefix)
        } else {
            // 组件级黑名单（如 ~/.ssh）按路径片段匹配，避免误伤 my.ssh-notes 之类目录。
            text.split(['/', '\\']).any(|seg| seg.eq_ignore_ascii_case(prefix))
        }
    })
}

fn under_allowed_root(canonical: &Path) -> bool {
    allowed_roots().iter().filter_map(|r| r.canonicalize().ok()).any(|root| canonical.starts_with(&root))
}

/// 把 `~` / `~/x` 展开为真实家目录；后端是唯一能做这件事的地方。
pub fn expand_tilde(raw: &str) -> Option<PathBuf> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed == "~" || trimmed == "~/" {
        return dirs::home_dir();
    }
    if let Some(rest) = trimmed.strip_prefix("~/").or_else(|| trimmed.strip_prefix("~\\")) {
        return dirs::home_dir().map(|home| home.join(rest));
    }
    Some(PathBuf::from(trimmed))
}

/// 解析大文件扫描根目录：展开 `~`、canonicalize、拒绝系统目录。
pub fn resolve_scan_path(raw: Option<&str>) -> CommandResult<PathBuf> {
    let home = dirs::home_dir();
    let candidate = match raw {
        None | Some("") => home.clone().ok_or_else(|| AppError::failed("无法定位用户主目录"))?,
        Some(input) => expand_tilde(input)
            .ok_or_else(|| AppError::invalid_input("扫描路径不能为空"))?,
    };

    if !candidate.exists() {
        return Err(AppError::not_found(format!("路径不存在: {}", crate::log_sanitize::sanitize(&candidate.to_string_lossy()))));
    }
    let canonical = candidate
        .canonicalize()
        .map_err(|e| AppError::failed(format!("无法解析路径: {}", e)))?;
    if is_denied(&canonical) {
        return Err(AppError::path_denied("该目录属于系统或受保护位置，不允许扫描"));
    }
    if !(under_allowed_root(&canonical) || home.map(|h| canonical.starts_with(&h)).unwrap_or(false)) {
        return Err(AppError::path_denied(
            "仅允许扫描用户主目录或临时目录之下的路径",
        ));
    }
    Ok(canonical)
}

/// 清理入口：确认声明目录 canonicalize 之后仍落在允许范围内，否则跳过。
fn approved_cleanup_root(declared: &str) -> Option<PathBuf> {
    let path = PathBuf::from(declared);
    if !path.exists() {
        return None;
    }
    let canonical = path.canonicalize().ok()?;
    if is_denied(&canonical) || !under_allowed_root(&canonical) {
        return None;
    }
    Some(canonical)
}

// 垃圾清理类别
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JunkCategory {
    pub id: String,
    pub name: String,
    pub description: String,
    pub paths: Vec<String>,
    pub size_bytes: u64,
    pub file_count: usize,
    pub risk_level: RiskLevel,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RiskLevel {
    Safe,      // 可安全清理 (临时文件、缓存)
    Moderate,  // 中等风险 (缩略图、最近文档)
    Risky,     // 高风险 (日志、数据库)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JunkReport {
    pub total_size_bytes: u64,
    pub total_files: usize,
    pub categories: Vec<JunkCategory>,
    pub scan_time_ms: u64,
    /// true 表示用户中途取消，`categories` 只是已扫描部分的结果
    pub cancelled: bool,
}

/// 扫描进度帧（T3-09）。只报"已扫完多少个目录、当前目录下已见多少个文件"，
/// 不预估剩余时间，也不插入合成进度点。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanProgress {
    pub scanned_paths: usize,
    pub total_paths: usize,
    pub current_path: String,
    pub current_path_files: usize,
    pub found_files: usize,
    pub found_bytes: u64,
    pub cancelled: bool,
    pub done: bool,
}

/// 进度帧的 Tauri 事件名。
pub const SCAN_PROGRESS_EVENT: &str = "sys://cleanup-scan-progress";

/// 扫描态机：扫描线程与 `cancel_junk_scan` 命令之间唯一的通道。
/// 测试各自 `ScanFlags::new()`，因此断言不受并行用例的全局状态干扰。
pub struct ScanFlags {
    state: AtomicU8,
}

/// 单字状态机 `空闲 → 扫描中 → 已请求取消`，收尾一律回到 `空闲`。
/// 用"一个原子字"而不是"运行 bool + 取消 bool"，是为了让认领与清取消标记落在
/// 同一次 CAS 里 —— 否则"先 CAS running、再清 cancel"会抹掉同一瞬间用户按下的取消。
const IDLE: u8 = 0;
const RUNNING: u8 = 1;
const CANCEL_REQUESTED: u8 = 2;

impl ScanFlags {
    pub const fn new() -> Self {
        Self {
            state: AtomicU8::new(IDLE),
        }
    }

    fn state(&self) -> u8 {
        self.state.load(Ordering::SeqCst)
    }

    /// 返回"请求时是否确有扫描在跑"，前端据此区分"已取消"与"没有可取消的扫描"。
    /// 只有确实处于某个扫描态时才写取消位；对 `空闲` 状态的请求不落任何标记，
    /// 因此残留的取消不会打到下一轮扫描上。
    pub fn request_cancel(&self) -> bool {
        let mut current = self.state();
        loop {
            match current {
                IDLE => return false,
                CANCEL_REQUESTED => return true,
                _ => match self.state.compare_exchange(
                    current,
                    CANCEL_REQUESTED,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                ) {
                    Ok(_) => return true,
                    Err(next) => current = next,
                },
            }
        }
    }

    fn cancelled(&self) -> bool {
        self.state() == CANCEL_REQUESTED
    }

    /// 认领这一轮扫描：只有"空闲 → 扫描中"的那次调用会成功，因此并发下发第二份
    /// 扫描会被直接拒掉。返回的 guard 离开作用域时复位，避免取消打到已结束的扫描上。
    /// guard 由调用方持有并搬进扫描线程 —— 从认领成功那一刻起，取消请求就不会落空。
    pub fn begin(&self) -> Option<ScanGuard<'_>> {
        self.state
            .compare_exchange(IDLE, RUNNING, Ordering::SeqCst, Ordering::SeqCst)
            .ok()
            .map(|_| ScanGuard { flags: self })
    }
}

/// 一轮扫描的认领凭证：见 `ScanFlags::begin`。
pub struct ScanGuard<'a> {
    flags: &'a ScanFlags,
}

impl ScanGuard<'_> {
    fn cancelled(&self) -> bool {
        self.flags.cancelled()
    }
}

impl Drop for ScanGuard<'_> {
    fn drop(&mut self) {
        self.flags.state.store(IDLE, Ordering::SeqCst);
    }
}

static JUNK_SCAN: ScanFlags = ScanFlags::new();

/// 扫描命令使用的进程级标志。
pub fn junk_scan_flags() -> &'static ScanFlags {
    &JUNK_SCAN
}

pub fn request_cancel_junk_scan() -> bool {
    JUNK_SCAN.request_cancel()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupResult {
    pub freed_bytes: u64,
    pub deleted_files: usize,
    pub failed_files: usize,
    /// 命中黑名单而整目录跳过的数量。
    pub skipped_paths: usize,
    pub errors: Vec<String>,
}

// 大文件项
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LargeFile {
    pub path: String,
    pub size_bytes: u64,
    /// unix 秒级时间戳
    pub modified_seconds: i64,
    pub is_dir: bool,
}

// 启动项
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartupItem {
    pub id: String,
    pub name: String,
    pub command: String,
    pub source: String,       // 注册表/LaunchAgents/cron 等
    pub enabled: bool,
    pub location: String,
}

// 获取系统垃圾目录
fn get_junk_paths() -> Vec<(String, String, String, RiskLevel)> {
    let mut paths: Vec<(String, String, String, RiskLevel)> = Vec::new();

    if let Some(home) = dirs::home_dir() {
        // 通用垃圾目录
        paths.push((
            "user_cache".to_string(),
            "用户缓存".to_string(),
            home.join(".cache").to_string_lossy().to_string(),
            RiskLevel::Safe,
        ));
        paths.push((
            "user_temp".to_string(),
            "用户临时文件".to_string(),
            std::env::temp_dir().join("z-biz-tool-sys").to_string_lossy().to_string(),
            RiskLevel::Safe,
        ));
        paths.push((
            "recent_files".to_string(),
            "最近文件列表".to_string(),
            home.join(".local/share/Recent").to_string_lossy().to_string(),
            RiskLevel::Moderate,
        ));
        paths.push((
            "thumbnail_cache".to_string(),
            "缩略图缓存".to_string(),
            home.join(".cache/thumbnails").to_string_lossy().to_string(),
            RiskLevel::Moderate,
        ));
        paths.push((
            "trash".to_string(),
            "回收站".to_string(),
            home.join(".local/share/Trash").to_string_lossy().to_string(),
            RiskLevel::Moderate,
        ));
        paths.push((
            "browser_cache".to_string(),
            "浏览器缓存".to_string(),
            home.join(".cache/google-chrome").to_string_lossy().to_string(),
            RiskLevel::Moderate,
        ));
        paths.push((
            "browser_cache".to_string(),
            "浏览器缓存".to_string(),
            home.join(".cache/BraveSoftware").to_string_lossy().to_string(),
            RiskLevel::Moderate,
        ));
        paths.push((
            "npm_cache".to_string(),
            "NPM 缓存".to_string(),
            home.join(".npm").to_string_lossy().to_string(),
            RiskLevel::Moderate,
        ));
    }

    // 系统级垃圾目录
    paths.push((
        "tmp".to_string(),
        "系统临时目录".to_string(),
        std::env::temp_dir().to_string_lossy().to_string(),
        RiskLevel::Safe,
    ));

    #[cfg(target_os = "macos")]
    {
        if let Some(home) = dirs::home_dir() {
            paths.push((
                "macos_logs".to_string(),
                "macOS 系统日志缓存".to_string(),
                home.join("Library/Logs").to_string_lossy().to_string(),
                RiskLevel::Risky,
            ));
            paths.push((
                "macos_caches".to_string(),
                "macOS 用户缓存".to_string(),
                home.join("Library/Caches").to_string_lossy().to_string(),
                RiskLevel::Moderate,
            ));
            paths.push((
                "macos_xcode".to_string(),
                "Xcode 派生数据".to_string(),
                home.join("Library/Developer/Xcode/DerivedData").to_string_lossy().to_string(),
                RiskLevel::Moderate,
            ));
            paths.push((
                "macos_simulator".to_string(),
                "iOS 模拟器临时文件".to_string(),
                home.join("Library/Developer/CoreSimulator/Caches").to_string_lossy().to_string(),
                RiskLevel::Moderate,
            ));
        }
        paths.push((
            "macos_sys_logs".to_string(),
            "macOS 系统日志".to_string(),
            "/private/var/log/asl".to_string(),
            RiskLevel::Risky,
        ));
        paths.push((
            "macos_sys_logs".to_string(),
            "macOS 系统日志".to_string(),
            "/Library/Logs/DiagnosticReports".to_string(),
            RiskLevel::Risky,
        ));
    }

    #[cfg(target_os = "windows")]
    {
        if let Ok(windir) = std::env::var("WINDIR") {
            paths.push((
                "win_temp".to_string(),
                "Windows 临时目录".to_string(),
                format!("{}\\Temp", windir),
                RiskLevel::Safe,
            ));
        }
        paths.push((
            "win_recent".to_string(),
            "Windows 最近文件".to_string(),
            "C:\\Users\\Default\\AppData\\Roaming\\Microsoft\\Windows\\Recent".to_string(),
            RiskLevel::Moderate,
        ));
        paths.push((
            "win_logs".to_string(),
            "Windows 日志".to_string(),
            "C:\\Windows\\Logs".to_string(),
            RiskLevel::Risky,
        ));
    }

    paths
}

// 计算目录大小
fn dir_size(path: &Path) -> (u64, usize) {
    let (size, count, _) = dir_size_with_progress(path, |_| true);
    (size, count)
}

/// 每累计这么多文件回调一次，让大目录扫描过程中进度条也能前进。
const PROGRESS_EVERY_FILES: usize = 500;

/// 回调返回 false 表示调用方要求中止遍历（T3-09 的取消）。
/// 返回 `(总字节, 文件数, 是否被提前中止)`：中止时给的是已累加到的部分，不谎报完整。
fn dir_size_with_progress(
    path: &Path,
    mut on_files: impl FnMut(usize) -> bool,
) -> (u64, usize, bool) {
    let mut size = 0u64;
    let mut count = 0usize;
    if !path.exists() {
        return (0, 0, false);
    }
    let mut entries = WalkDir::new(path).into_iter().filter_map(Result::ok);
    loop {
        let Some(entry) = entries.next() else {
            return (size, count, false);
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        size += meta.len();
        count += 1;
        if count.is_multiple_of(PROGRESS_EVERY_FILES) && !on_files(count) {
            return (size, count, true);
        }
    }
}

/// 一个待扫描目标：`(类别 id, 名称, 已过白名单的路径, 风险等级)`
pub type ScanTarget = (String, String, PathBuf, RiskLevel);

/// 逐目录扫描并回调进度；取消位置位后在下一个检查点（含单个大目录内部）停下，
/// 已扫描出的部分照常返回（`cancelled = true`），不丢弃结果也不谎报完整。
/// `guard` 是 `ScanFlags::begin` 的认领凭证：没有它就无法开始一轮扫描。
pub fn scan_junk_with_progress(
    on_progress: &mut dyn FnMut(&ScanProgress),
    guard: &ScanGuard,
) -> JunkReport {
    // 先过一遍白名单：`total_paths` 必须是真实会扫描的目录数，否则进度条会假 advance。
    let targets: Vec<ScanTarget> = get_junk_paths()
        .into_iter()
        .filter_map(|(id, name, path_str, risk)| {
            approved_cleanup_root(&path_str).map(|path| (id, name, path, risk))
        })
        .collect();
    scan_targets(&targets, on_progress, guard)
}

/// 真正的遍历内核。与"扫描哪些目录"解耦，因此测试可以用临时目录跑完整进度序列，
/// 不必（也不允许）碰用户真实的缓存目录。
pub fn scan_targets(
    targets: &[ScanTarget],
    on_progress: &mut dyn FnMut(&ScanProgress),
    guard: &ScanGuard,
) -> JunkReport {
    let start = std::time::Instant::now();
    let mut grouped: Vec<JunkCategory> = Vec::new();
    let total_paths = targets.len();
    let mut found_files = 0usize;
    let mut found_bytes = 0u64;
    let mut cancelled = false;
    let mut scanned_paths = 0usize;

    // 没有目录可扫时不必先发一帧 0/0：直接由结束帧交代 done。
    if total_paths > 0 {
        emit_progress(
            on_progress,
            ScanProgress {
                scanned_paths: 0,
                total_paths,
                current_path: String::new(),
                current_path_files: 0,
                found_files: 0,
                found_bytes: 0,
                cancelled: guard.cancelled(),
                done: false,
            },
        );
    }

    for (index, (id, name, path, risk)) in targets.iter().enumerate() {
        if guard.cancelled() {
            cancelled = true;
            break;
        }
        let shown = path.to_string_lossy().into_owned();
        let (size, count, stopped_early) = {
            let on_progress = &mut *on_progress;
            let shown = shown.clone();
            dir_size_with_progress(path, |seen| {
                emit_progress(
                    on_progress,
                    ScanProgress {
                        scanned_paths: index,
                        total_paths,
                        current_path: shown.clone(),
                        current_path_files: seen,
                        found_files,
                        found_bytes,
                        cancelled: false,
                        done: false,
                    },
                );
                !guard.cancelled()
            })
        };
        if stopped_early {
            cancelled = true;
        }

        if size > 0 {
            match grouped.iter().position(|c| c.id == *id) {
                Some(pos) => {
                    let entry = &mut grouped[pos];
                    entry.paths.push(shown.clone());
                    entry.size_bytes += size;
                    entry.file_count += count;
                }
                None => grouped.push(JunkCategory {
                    id: id.clone(),
                    name: name.clone(),
                    description: format!("自动检测到的 {} 目录", name),
                    paths: vec![shown.clone()],
                    size_bytes: size,
                    file_count: count,
                    risk_level: risk.clone(),
                }),
            }
            found_files += count;
            found_bytes += size;
        }

        scanned_paths = index + 1;
        emit_progress(
            on_progress,
            ScanProgress {
                scanned_paths,
                total_paths,
                current_path: shown,
                current_path_files: count,
                found_files,
                found_bytes,
                cancelled,
                done: false,
            },
        );
        if cancelled {
            break;
        }
    }

    // 按大小降序；同大小再按 id，保证两次扫描给出同样顺序
    let mut categories = grouped;
    categories.sort_by(|a, b| {
        b.size_bytes
            .cmp(&a.size_bytes)
            .then_with(|| a.id.cmp(&b.id))
    });

    let total_size_bytes: u64 = categories.iter().map(|c| c.size_bytes).sum();
    let total_files: usize = categories.iter().map(|c| c.file_count).sum();

    emit_progress(
        on_progress,
        ScanProgress {
            scanned_paths,
            total_paths,
            current_path: String::new(),
            current_path_files: 0,
            found_files: total_files,
            found_bytes: total_size_bytes,
            cancelled,
            done: true,
        },
    );

    JunkReport {
        total_size_bytes,
        total_files,
        categories,
        scan_time_ms: start.elapsed().as_millis() as u64,
        cancelled,
    }
}

fn emit_progress(on_progress: &mut dyn FnMut(&ScanProgress), frame: ScanProgress) {
    on_progress(&frame);
}

// 清理选定的类别
pub fn cleanup_categories(ids: &[String]) -> CleanupResult {
    let mut result = CleanupResult {
        freed_bytes: 0,
        deleted_files: 0,
        failed_files: 0,
        skipped_paths: 0,
        errors: Vec::new(),
    };

    let raw_paths = get_junk_paths();
    let target_paths: Vec<PathBuf> = raw_paths
        .into_iter()
        .filter(|(id, _, _, _)| ids.contains(id))
        .filter_map(|(_, _, path, _)| match approved_cleanup_root(&path) {
            Some(root) => Some(root),
            None => {
                result.errors.push(crate::log_sanitize::sanitize(&format!(
                    "已跳过受保护目录: {}",
                    path
                )));
                result.skipped_paths += 1;
                None
            }
        })
        .collect();

    for path in target_paths {

        let (size_before, _count_before) = dir_size(&path);

        for entry in WalkDir::new(&path).into_iter().filter_map(Result::ok) {
            if entry.file_type().is_file() {
                if let Err(e) = fs::remove_file(entry.path()) {
                    result.failed_files += 1;
                    if result.errors.len() < 10 {
                        result.errors.push(crate::log_sanitize::sanitize(&format!(
                            "{}: {}",
                            entry.path().display(),
                            e
                        )));
                    }
                } else {
                    result.deleted_files += 1;
                }
            }
        }

        // 清理空目录
        if let Ok(rd) = WalkDir::new(&path).into_iter().collect::<Result<Vec<_>, _>>() {
            for entry in rd.iter().rev() {
                if entry.file_type().is_dir() && entry.path() != path.as_path() {
                    let _ = fs::remove_dir(entry.path());
                }
            }
        }

        let (size_after, _) = dir_size(&path);
        result.freed_bytes += size_before.saturating_sub(size_after);
    }

    result
}

// 扫描大文件
pub fn find_large_files(path: &Path, min_size: u64, limit: usize) -> Vec<LargeFile> {
    let mut results = Vec::new();

    for entry in WalkDir::new(path)
        .max_depth(12)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
        if let Ok(meta) = entry.metadata() {
            let size = meta.len();
            if size >= min_size {
                let modified = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);

                results.push(LargeFile {
                    path: entry.path().to_string_lossy().to_string(),
                    size_bytes: size,
                    modified_seconds: modified,
                    is_dir: false,
                });
            }
        }
    }

    results.sort_by_key(|f| std::cmp::Reverse(f.size_bytes));
    results.truncate(limit);
    results
}

// 获取启动项（跨平台简化实现）
pub fn get_startup_items() -> Vec<StartupItem> {
    let mut items = Vec::new();

    #[cfg(target_os = "macos")]
    {
        if let Ok(output) = std::process::Command::new("osascript")
            .args(["-e", "tell application \"System Events\" to get name of every login item"])
            .output()
        {
            if let Ok(s) = String::from_utf8(output.stdout) {
                for (i, name) in s.split(", ").enumerate() {
                    let name = name.trim().to_string();
                    if !name.is_empty() {
                        items.push(StartupItem {
                            id: format!("macos-login-{}", i),
                            name: name.clone(),
                            command: name,
                            source: "Login Items".to_string(),
                            enabled: true,
                            location: "~/Library/Application Support".to_string(),
                        });
                    }
                }
            }
        }

        // LaunchAgents
        if let Some(home) = dirs::home_dir() {
            let launch_agents = home.join("Library/LaunchAgents");
            if launch_agents.exists() {
                if let Ok(entries) = fs::read_dir(&launch_agents) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.extension().and_then(|s| s.to_str()) == Some("plist") {
                            let name = path.file_name()
                                .and_then(|s| s.to_str())
                                .unwrap_or("")
                                .to_string();
                            items.push(StartupItem {
                                id: format!("macos-agent-{}", name),
                                name: name.clone(),
                                command: path.to_string_lossy().to_string(),
                                source: "LaunchAgent".to_string(),
                                enabled: true,
                                location: launch_agents.to_string_lossy().to_string(),
                            });
                        }
                    }
                }
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        // 读取注册表 Run 项
        if let Ok(output) = std::process::Command::new("reg")
            .args(["query", "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run"])
            .output()
        {
            if let Ok(s) = String::from_utf8(output.stdout) {
                for line in s.lines() {
                    if line.contains("REG_SZ") {
                        let parts: Vec<&str> = line.split_whitespace().collect();
                        if parts.len() >= 3 {
                            let name = parts[0].to_string();
                            let command = parts[2..].join(" ");
                            items.push(StartupItem {
                                id: format!("win-run-{}", name),
                                name,
                                command,
                                source: "Registry Run".to_string(),
                                enabled: true,
                                location: "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run".to_string(),
                            });
                        }
                    }
                }
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
        // .desktop 文件位于 ~/.config/autostart/
        if let Some(home) = dirs::home_dir() {
            let autostart = home.join(".config/autostart");
            if autostart.exists() {
                if let Ok(entries) = fs::read_dir(&autostart) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.extension().and_then(|s| s.to_str()) == Some("desktop") {
                            let name = path.file_name()
                                .and_then(|s| s.to_str())
                                .unwrap_or("")
                                .to_string();
                            items.push(StartupItem {
                                id: format!("linux-autostart-{}", name),
                                name,
                                command: path.to_string_lossy().to_string(),
                                source: "XDG Autostart".to_string(),
                                enabled: true,
                                location: autostart.to_string_lossy().to_string(),
                            });
                        }
                    }
                }
            }
        }

        // systemd 用户服务
        if let Some(home) = dirs::home_dir() {
            let systemd = home.join(".config/systemd/user");
            if systemd.exists() {
                if let Ok(entries) = fs::read_dir(&systemd) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if let Some(ext) = path.extension() {
                            if ext == "service" {
                                let name = path.file_name()
                                    .and_then(|s| s.to_str())
                                    .unwrap_or("")
                                    .to_string();
                                items.push(StartupItem {
                                    id: format!("linux-systemd-{}", name),
                                    name,
                                    command: path.to_string_lossy().to_string(),
                                    source: "systemd user".to_string(),
                                    enabled: true,
                                    location: systemd.to_string_lossy().to_string(),
                                });
                            }
                        }
                    }
                }
            }
        }
    }

    items
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    static SEQ: AtomicUsize = AtomicUsize::new(0);

    /// 每个用例拿自己的临时目录，避免并行用例互相看到对方的文件。
    fn fixture(tag: &str) -> PathBuf {
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "zsys-scan-{}-{}-{}",
            std::process::id(),
            tag,
            n
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_files(dir: &Path, count: usize, size: usize) {
        for i in 0..count {
            std::fs::write(dir.join(format!("f{i}.bin")), vec![b'x'; size]).unwrap();
        }
    }

    fn target(dir: &Path, id: &str) -> ScanTarget {
        (
            id.to_string(),
            id.to_string(),
            dir.to_path_buf(),
            RiskLevel::Safe,
        )
    }

    #[test]
    fn progress_frames_are_monotonic_and_end_with_a_done_frame() {
        let a = fixture("mono-a");
        let b = fixture("mono-b");
        write_files(&a, 3, 100);
        write_files(&b, 2, 50);

        let flags = ScanFlags::new();
        let guard = flags.begin().expect("无并发时应能认领扫描");
        let mut frames = Vec::new();
        let report = scan_targets(
            &[target(&a, "a"), target(&b, "b")],
            &mut |f| frames.push(f.clone()),
            &guard,
        );

        assert_eq!(report.total_files, 5);
        assert_eq!(report.total_size_bytes, 400);
        assert!(!report.cancelled);
        assert_eq!(report.categories[0].id, "a");
        assert_eq!(report.categories[0].file_count, 3);

        let last = frames.last().unwrap();
        assert!(last.done && !last.cancelled);
        assert_eq!((last.scanned_paths, last.total_paths), (2, 2));
        assert_eq!((last.found_files, last.found_bytes), (5, 400));

        // 进度只前进不倒退，且不会超过 100%
        let mut prev = 0usize;
        for f in &frames {
            assert!(
                f.scanned_paths >= prev && f.scanned_paths <= f.total_paths,
                "进度不单调：{f:?}"
            );
            assert!(f.found_bytes <= 400 && f.found_files <= 5);
            prev = f.scanned_paths;
        }
        // 起始帧 + 每个目录一帧 + 结束帧
        assert!(frames.len() >= 4, "帧数过少：{}", frames.len());
        assert!(!frames[0].done && frames[0].scanned_paths == 0);

        let _ = std::fs::remove_dir_all(a);
        let _ = std::fs::remove_dir_all(b);
    }

    #[test]
    fn empty_target_list_is_done_from_the_first_frame() {
        let flags = ScanFlags::new();
        let guard = flags.begin().expect("无并发时应能认领扫描");
        let mut frames = Vec::new();
        let report = scan_targets(&[], &mut |f| frames.push(f.clone()), &guard);
        assert_eq!(frames.len(), 1);
        assert!(frames[0].done);
        assert_eq!(frames[0].total_paths, 0);
        assert!(report.categories.is_empty());
        assert!(!report.cancelled);
    }

    #[test]
    fn cancel_between_directories_keeps_the_partial_report() {
        let a = fixture("between-a");
        let b = fixture("between-b");
        write_files(&a, 3, 100);
        write_files(&b, 4, 1000);

        let flags = ScanFlags::new();
        let guard = flags.begin().expect("无并发时应能认领扫描");
        let mut frames = Vec::new();
        let report = scan_targets(
            &[target(&a, "a"), target(&b, "b")],
            &mut |f| {
                // 第一个目录扫完那一刻请求取消
                if f.scanned_paths == 1 && !f.current_path.is_empty() {
                    assert!(flags.request_cancel(), "扫描进行中请求取消应被受理");
                }
                frames.push(f.clone());
            },
            &guard,
        );

        assert!(report.cancelled, "取消必须体现在报告里，不能谎报完整");
        assert_eq!(report.categories.len(), 1, "b 不应被扫描");
        assert_eq!(report.categories[0].id, "a");
        assert_eq!(report.total_files, 3);
        assert_eq!(report.total_size_bytes, 300);
        let last = frames.last().unwrap();
        assert!(last.done && last.cancelled);
        assert_eq!((last.scanned_paths, last.total_paths), (1, 2));

        let _ = std::fs::remove_dir_all(a);
        let _ = std::fs::remove_dir_all(b);
    }

    #[test]
    fn cancel_takes_effect_inside_one_huge_directory() {
        let big = fixture("huge");
        // 超过 PROGRESS_EVERY_FILES，才能验证"单个大目录内部"也能停
        write_files(&big, PROGRESS_EVERY_FILES * 2 + 1, 1);
        let small = fixture("after-huge");
        write_files(&small, 1, 1);

        let flags = ScanFlags::new();
        let guard = flags.begin().expect("无并发时应能认领扫描");
        let report = scan_targets(
            &[target(&big, "big"), target(&small, "small")],
            &mut |f| {
                if f.current_path_files == PROGRESS_EVERY_FILES {
                    flags.request_cancel();
                }
            },
            &guard,
        );

        assert!(report.cancelled);
        assert_eq!(report.categories.len(), 1);
        // 只累加到检查点为止：数量是真实看到的，不是整个目录的
        assert_eq!(report.categories[0].file_count, PROGRESS_EVERY_FILES);
        assert_eq!(report.total_files, PROGRESS_EVERY_FILES);
        assert_eq!(report.total_size_bytes, PROGRESS_EVERY_FILES as u64);

        let _ = std::fs::remove_dir_all(big);
        let _ = std::fs::remove_dir_all(small);
    }

    #[test]
    fn a_round_is_claimed_once_and_releases_on_drop() {
        let dir = fixture("claim");
        write_files(&dir, 2, 10);
        let flags = ScanFlags::new();
        assert!(!flags.request_cancel(), "没有扫描在跑时不应假称已取消");

        let guard = flags.begin().expect("首轮应能认领");
        assert!(
            flags.begin().is_none(),
            "认领凭证存活期间第二次认领必须失败，否则会并发扫描"
        );

        let mut frames = Vec::new();
        scan_targets(&[target(&dir, "r")], &mut |f| frames.push(f.clone()), &guard);
        assert!(frames.iter().all(|f| !f.cancelled), "未取消时帧里不应出现取消态");

        // 认领之后按下的取消必须立刻对扫描线程可见
        assert!(flags.request_cancel(), "认领后取消请求必须被受理");
        assert!(guard.cancelled());

        // 丢掉凭证即复位：否则下一轮取消会打到空处、并继承上一轮的取消标记
        drop(guard);
        assert!(!flags.request_cancel(), "结束后必须复位，否则取消会打到空处");
        let again = flags.begin().expect("复位后应能再次认领");
        assert!(!again.cancelled(), "新一轮不应继承上一轮的取消标记");

        let _ = std::fs::remove_dir_all(dir);
    }

    /// 关键不变量：取消只在"已认领"之后才有效。命令在下发 `spawn_blocking` 之前
    /// 就必须认领，否则从命令返回到扫描线程起跑之间的窗口里，用户的取消会落空。
    #[test]
    fn cancel_requested_before_the_claim_is_ignored_by_the_next_scan() {
        let dir = fixture("pre-cancel");
        write_files(&dir, 2, 10);
        let flags = ScanFlags::new();
        assert!(!flags.request_cancel(), "未认领时的取消请求不应被接受");

        let guard = flags.begin().expect("首轮应能认领");
        let mut frames = Vec::new();
        let report = scan_targets(
            &[target(&dir, "r")],
            &mut |f| frames.push(f.clone()),
            &guard,
        );
        assert!(!report.cancelled, "上一轮残留的取消标记不能污染这一轮");
        assert_eq!(report.total_files, 2);
        assert!(frames.last().unwrap().done && !frames.last().unwrap().cancelled);

        let _ = std::fs::remove_dir_all(dir);
    }

    /// 并发压力：认领必须串行化（同时最多一轮扫描），高频取消收敛后不得留下残留态。
    /// 交错次数只打印不断言 —— 那是调度而非产品性质，断言它会造成偶发假失败。
    #[test]
    fn concurrent_claims_never_overlap_and_leave_no_residue() {
        use std::sync::atomic::AtomicBool;
        use std::sync::{Arc, Barrier};

        const CLAIMERS: usize = 4;
        const ROUNDS: usize = 300;

        let flags = Arc::new(ScanFlags::new());
        // held 必须"先清再放凭证"，否则另一线程会在凭证释放后、held 清零前误判重叠
        let held = Arc::new(AtomicBool::new(false));
        let barrier = Arc::new(Barrier::new(CLAIMERS + 1));

        let claimers: Vec<_> = (0..CLAIMERS)
            .map(|_| {
                let flags = Arc::clone(&flags);
                let held = Arc::clone(&held);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    let mut rejected = 0;
                    for _ in 0..ROUNDS {
                        match flags.begin() {
                            Some(guard) => {
                                assert!(
                                    !held.swap(true, Ordering::SeqCst),
                                    "两轮扫描不得同时处于认领态"
                                );
                                std::thread::yield_now();
                                held.store(false, Ordering::SeqCst);
                                drop(guard);
                            }
                            None => rejected += 1,
                        }
                    }
                    rejected
                })
            })
            .collect();

        let canceller = {
            let flags = Arc::clone(&flags);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                let mut accepted = 0;
                for _ in 0..(CLAIMERS * ROUNDS) {
                    accepted += flags.request_cancel() as usize;
                }
                accepted
            })
        };

        let rejected: usize = claimers
            .into_iter()
            .map(|h| h.join().expect("认领线程不应 panic"))
            .sum();
        let accepted = canceller.join().expect("取消线程不应 panic");
        println!("[并发压力] 认领被拒 {rejected} 次 / 取消被受理 {accepted} 次");

        // 全部线程收敛后必须回到空闲态，且不带任何取消残留
        let fresh = flags.begin().expect("收敛后应能重新认领");
        assert!(!fresh.cancelled(), "并发取消不得在空闲态留下残留标记");
        drop(fresh);
    }

    /// 打穿到生产静态 `JUNK_SCAN` + 真实白名单路径（只读遍历，不删除任何文件）：
    /// 扫描进行中第二份扫描必须被拒，取消必须让真实扫描在下一个目录前停下。
    #[test]
    fn real_scan_rejects_a_second_claim_and_honours_cancel() {
        let flags = junk_scan_flags();
        assert!(
            !flags.request_cancel(),
            "测试开始前不应有别的扫描占着全局标志（若有说明上一轮没复位）"
        );

        let guard = flags.begin().expect("首轮真实扫描应能认领");
        assert!(
            flags.begin().is_none(),
            "扫描进行中第二份扫描必须被拒，否则会并发遍历"
        );

        let mut total_paths = 0usize;
        let mut scanned_at_cancel = 0usize;
        let report = scan_junk_with_progress(
            &mut |f| {
                total_paths = f.total_paths;
                if !f.done && f.scanned_paths >= 1 {
                    scanned_at_cancel = f.scanned_paths;
                    assert!(flags.request_cancel(), "扫描中请求取消必须被受理");
                }
            },
            &guard,
        );

        println!(
            "[真实扫描] 白名单目录 {total_paths} 个，取消时已扫 {scanned_at_cancel} 个，\
             结果 {} 类 / {} 文件 / {} 字节，cancelled={}",
            report.categories.len(),
            report.total_files,
            report.total_size_bytes,
            report.cancelled
        );

        if total_paths >= 2 {
            assert!(
                report.cancelled,
                "白名单有 {total_paths} 个目录，首个目录后请求的取消必须生效"
            );
            assert!(
                report.categories.len() < total_paths,
                "取消后不应把剩余目录也扫完：{} / {total_paths}",
                report.categories.len()
            );
        }
        // 取消的报告仍要诚实：done 帧带 cancelled，且计数不为负
        assert!(!report.categories.is_empty() || report.total_files == 0);

        drop(guard);
        assert!(
            junk_scan_flags().begin().is_some(),
            "真实扫描结束后全局标志必须复位"
        );
    }

    /// 进度帧是跨语言契约：前端按 camelCase 字段名读，事件名也写在 `ipc_contract.ts`。
    /// 任一侧改名而另一侧没跟上，进度条会静默失效，所以在这里锁死。
    #[test]
    fn progress_frame_contract_matches_the_frontend_ipc_file() {
        let frame = ScanProgress {
            scanned_paths: 1,
            total_paths: 2,
            current_path: "/tmp/x".into(),
            current_path_files: 3,
            found_files: 4,
            found_bytes: 5,
            cancelled: false,
            done: true,
        };
        let json = serde_json::to_string(&frame).unwrap();
        let keys = [
            "scannedPaths",
            "totalPaths",
            "currentPath",
            "currentPathFiles",
            "foundFiles",
            "foundBytes",
            "cancelled",
            "done",
        ];
        for key in keys {
            assert!(json.contains(&format!("\"{key}\"")), "进度帧缺字段 {key}：{json}");
        }
        assert!(!json.contains('_'), "进度帧必须全 camelCase：{json}");

        let ts = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../src/ipc_contract.ts");
        let contract = fs::read_to_string(&ts).expect("前端 IPC 契约文件必须存在");
        assert!(
            contract.contains(&format!("\"{SCAN_PROGRESS_EVENT}\"")),
            "前端未登记事件名 {SCAN_PROGRESS_EVENT}"
        );
        for key in keys {
            assert!(
                contract.contains(&format!("{key}:")),
                "前端 ScanProgress 缺字段 {key}"
            );
        }
    }
}
