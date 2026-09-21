// 系统清理模块：垃圾文件扫描、清理、大文件查找等
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

// 垃圾清理类别
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JunkCategory {
    pub id: String,
    pub name: String,
    pub description: String,
    pub paths: Vec<String>,
    pub size: u64,
    pub file_count: usize,
    pub risk_level: RiskLevel,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum RiskLevel {
    Safe,      // 可安全清理 (临时文件、缓存)
    Moderate,  // 中等风险 (缩略图、最近文档)
    Risky,     // 高风险 (日志、数据库)
}

impl RiskLevel {
    fn as_str(&self) -> &'static str {
        match self {
            RiskLevel::Safe => "safe",
            RiskLevel::Moderate => "moderate",
            RiskLevel::Risky => "risky",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JunkReport {
    pub total_size: u64,
    pub total_files: usize,
    pub categories: Vec<JunkCategory>,
    pub scan_time_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CleanupResult {
    pub freed_bytes: u64,
    pub deleted_files: usize,
    pub failed_files: usize,
    pub errors: Vec<String>,
}

// 大文件项
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LargeFile {
    pub path: String,
    pub size: u64,
    pub modified: i64,        // unix timestamp
    pub is_dir: bool,
}

// 启动项
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    let mut size = 0u64;
    let mut count = 0usize;
    if !path.exists() {
        return (0, 0);
    }
    for entry in WalkDir::new(path).into_iter().filter_map(Result::ok) {
        if entry.file_type().is_file() {
            if let Ok(meta) = entry.metadata() {
                size += meta.len();
                count += 1;
            }
        }
    }
    (size, count)
}

// 扫描垃圾文件
pub fn scan_junk() -> JunkReport {
    let start = std::time::Instant::now();
    let mut categories = Vec::new();
    let raw_paths = get_junk_paths();

    // 用 id 去重（同一个 id 可能多个路径）
    use std::collections::HashMap;
    let mut grouped: HashMap<String, JunkCategory> = HashMap::new();

    for (id, name, path_str, risk) in raw_paths {
        let path = PathBuf::from(&path_str);
        let (size, count) = dir_size(&path);

        if size == 0 {
            continue;
        }

        let entry = grouped.entry(id.clone()).or_insert_with(|| JunkCategory {
            id: id.clone(),
            name: name.clone(),
            description: format!("自动检测到的 {} 目录", name),
            paths: Vec::new(),
            size: 0,
            file_count: 0,
            risk_level: risk.clone(),
        });
        entry.paths.push(path_str);
        entry.size += size;
        entry.file_count += count;
    }

    for cat in grouped.values() {
        categories.push(cat.clone());
    }

    // 按大小降序
    categories.sort_by(|a, b| b.size.cmp(&a.size));

    let total_size: u64 = categories.iter().map(|c| c.size).sum();
    let total_files: usize = categories.iter().map(|c| c.file_count).sum();

    JunkReport {
        total_size,
        total_files,
        categories,
        scan_time_ms: start.elapsed().as_millis() as u64,
    }
}

// 清理选定的类别
pub fn cleanup_categories(ids: &[String]) -> CleanupResult {
    let mut result = CleanupResult {
        freed_bytes: 0,
        deleted_files: 0,
        failed_files: 0,
        errors: Vec::new(),
    };

    let raw_paths = get_junk_paths();
    let target_paths: Vec<String> = raw_paths
        .into_iter()
        .filter(|(id, _, _, _)| ids.contains(id))
        .map(|(_, _, path, _)| path)
        .collect();

    for path_str in target_paths {
        let path = PathBuf::from(&path_str);
        if !path.exists() {
            continue;
        }

        let (size_before, _count_before) = dir_size(&path);

        for entry in WalkDir::new(&path).into_iter().filter_map(Result::ok) {
            if entry.file_type().is_file() {
                if let Err(e) = fs::remove_file(entry.path()) {
                    result.failed_files += 1;
                    if result.errors.len() < 10 {
                        result.errors.push(format!("{}: {}", entry.path().display(), e));
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
pub fn find_large_files(path: &str, min_size: u64, limit: usize) -> Vec<LargeFile> {
    let mut results = Vec::new();
    let path_buf = PathBuf::from(path);

    if !path_buf.exists() {
        return results;
    }

    for entry in WalkDir::new(&path_buf)
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
                    size,
                    modified,
                    is_dir: false,
                });
            }
        }
    }

    results.sort_by(|a, b| b.size.cmp(&a.size));
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