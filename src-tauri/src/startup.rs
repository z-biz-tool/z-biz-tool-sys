//! 启动项操作（T3-08）：禁用 / 删除 / 恢复。
//!
//! 这一条是整个方案里唯一会**移动用户真实文件**的能力，所以边界写死在这里：
//!
//! 1. **前端只给 id，不给路径。** 路径由后端按 `id` 前缀重新解析回"某一枚就在受白名单目录里的普通文件"。
//!    渲染进程一旦能直接提交路径，这个命令就退化成了"移动任意文件"的通道。
//! 2. **只有文件型来源可操作**（macOS 的 `~/Library/LaunchAgents`、Linux 的 `~/.config/autostart`）。
//!    Login Items 要走 System Events 自动化授权、Windows 的注册表 Run 键要写注册表、systemd 单元要调
//!    `systemctl` —— 三条都拿不到能覆盖它的测试，就直接返回 UNSUPPORTED，而不是上一段没测过的改写代码。
//! 3. **不解析文件内容，也绝不物理删除。** 三种动作都只是一次 `rename`：读不懂 plist 不影响判定，
//!    也不可能被写坏；"删除"同样把原件移进备份目录，只是不再列进界面。
//! 4. **不覆盖。** 目标位置已有同名文件时一律拒绝。`fs::rename` 在 unix 上是静默覆盖语义，
//!    少这道闸就会出现"禁用把上一份备份顶掉了"这种只有事后才能发现的丢数据。
//! 5. **逐字确认。** 三个命令都要求 `confirm_name` 与文件名完全一致，前端确认框里显示要核对的那串。

use crate::cleanup::{canonicalize_clean, is_denied, StartupItem};
use crate::error::{AppError, CommandResult};
use crate::log_sanitize::sanitize;
use serde::Serialize;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use tauri::AppHandle;

/// 备份根目录的名字，落在应用数据目录下（与历史/告警同址，用户好找）。
pub const BACKUP_DIR_NAME: &str = "startup-backup";
/// 启动项文件本该是几 KB。大到这个量级说明认错了东西，不去动它。
pub const MAX_STARTUP_FILE_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum StartupAction {
    Disable,
    Remove,
    Restore,
}

impl StartupAction {
    fn label(self) -> &'static str {
        match self {
            Self::Disable => "禁用",
            Self::Remove => "删除",
            Self::Restore => "恢复",
        }
    }
}

/// 一个可操作来源：id 前缀 + 真实扫描目录 + 文件后缀，以及它自己的两个落点。
#[derive(Debug, Clone)]
pub struct SourceDirs {
    pub prefix: String,
    pub extension: String,
    /// 启动目录（live 文件所在）
    pub dir: PathBuf,
    /// 备份根。`app_data_dir()` 解析不出来时是 None（与 `history::store_for` 同一口径：
    /// 宁可列不出已禁用项，也不给一个"没地方放"的路径去当移动目标）。
    backup_root: Option<PathBuf>,
}

impl SourceDirs {
    /// 禁用落点：仍然列进界面，可一键恢复
    pub fn backup_dir(&self) -> Option<PathBuf> {
        self.backup_root.as_ref().map(|root| root.join(&self.prefix))
    }

    /// 删除落点：不列进界面，但文件仍在（本应用不做物理删除）
    pub fn out_dir(&self) -> Option<PathBuf> {
        self.backup_dir().map(|dir| dir.join("out"))
    }
}

#[derive(Debug, Clone, Default)]
pub struct Layout {
    pub sources: Vec<SourceDirs>,
}

/// 平台 → 能动的来源。新增一处就要同步 `ipc_contract.ts` 里给界面看的说明。
fn platform_sources(home: &Path) -> Vec<(String, PathBuf, String)> {
    let mut out: Vec<(String, PathBuf, String)> = Vec::new();

    #[cfg(target_os = "macos")]
    {
        out.push((
            "macos-agent".to_string(),
            home.join("Library/LaunchAgents"),
            "plist".to_string(),
        ));
    }
    #[cfg(target_os = "linux")]
    {
        // XDG autostart 是纯文件判定：文件在目录里就会被会话管理器读，移走即失效，移回即恢复。
        out.push((
            "linux-autostart".to_string(),
            home.join(".config/autostart"),
            "desktop".to_string(),
        ));
    }
    // Windows 的启动项在注册表 Run 键里；没有可覆盖的写入通路，就不摆一个能点的按钮。
    let _ = &out;
    out
}

/// 把"来源目录 + 备份根"拼成一份完整布局。备份落点按前缀分目录，避免不同来源同名互相顶掉。
fn build_layout(sources: Vec<(String, PathBuf, String)>, backup_root: Option<&Path>) -> Layout {
    let backup_root = backup_root.map(|root| root.to_path_buf());
    Layout {
        sources: sources
            .into_iter()
            .map(|(prefix, dir, extension)| SourceDirs {
                backup_root: backup_root.clone(),
                prefix,
                extension,
                dir,
            })
            .collect(),
    }
}

/// 这一枚 id 落在不落在可操作来源里。扫描器拿它给 `operable` 打标，界面据此决定给不给按钮 ——
/// 口径只有这一处，两边各判一次就会出现"按钮点得下去但后端回 UNSUPPORTED"。
pub fn id_is_operable(id: &str) -> bool {
    match dirs::home_dir() {
        Some(home) => platform_sources(&home)
            .into_iter()
            .any(|(prefix, _, _)| id.starts_with(&format!("{prefix}-"))),
        None => false,
    }
}

/// 生产入口。`backup_root` 由 command 从 `app_data_dir()` 解析后传进来 ——
/// 模块自己不猜应用数据目录在哪，两处各猜一份迟早和 `history.rs` 对不上。
pub fn layout_for(backup_root: Option<&Path>, home: Option<&Path>) -> Layout {
    build_layout(home.map(platform_sources).unwrap_or_default(), backup_root)
}

/// 备份根解析不出来时该怎么说。动文件之前必须有个确切的地方可放，"大概能建个目录"不算。
fn no_backup_root() -> AppError {
    AppError::failed("无法确定本应用的备份目录，启动项操作已取消").with_detail("app_data_dir")
}

/// 备份根：与历史/告警同址（`app_data_dir()/startup-backup`）。
/// 这里**不创建目录** —— 只是把路径定下来，真要落点时 `hand_over` 才建，
/// 免得"刷新一下列表"就在应用数据目录里留下一个从没用过的空目录。
pub fn backup_root_for(app: &AppHandle) -> Option<PathBuf> {
    use tauri::Manager;
    match app.path().app_data_dir() {
        Ok(dir) => Some(dir.join(BACKUP_DIR_NAME)),
        Err(e) => {
            eprintln!("[startup] 无法解析应用数据目录: {}", sanitize(&e.to_string()));
            None
        }
    }
}

/// command 侧的唯一入口：解析备份根 → 拼本机布局 → 执行。
/// 三家命令都走这里，避免"禁用做了校验、删除漏了一步"这种只有分别改两边才会出现的漂移。
pub fn run(app: &AppHandle, action: StartupAction, id: &str, confirm_name: &str) -> CommandResult<StartupOutcome> {
    let root = backup_root_for(app).ok_or_else(no_backup_root)?;
    let layout = layout_for(Some(&root), dirs::home_dir().as_deref());
    operate(&layout, action, id, confirm_name)
}

/// 界面那一页的数据。备份根解析不出来时仍然给在位项（只是看不到已禁用项），不让整个页签空白。
pub fn page(app: &AppHandle) -> Vec<StartupItem> {
    list_with_backup(backup_root_for(app).as_deref(), dirs::home_dir().as_deref())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartupOutcome {
    pub id: String,
    pub name: String,
    pub action: StartupAction,
    /// 动之前的位置（canonical 后的真身，用户照着能在访达里找到）
    pub from_path: String,
    /// 动之后的位置
    pub to_path: String,
    /// 移动了多少字节：与源文件大小一致，用来证明"整份搬走"而不是留了半个
    pub bytes: u64,
    /// 操作后这一项还会不会出现在列表里（删除不再列，但文件仍在 `out_dir`）
    pub listed: bool,
    pub message: String,
}

/// `~/Library/LaunchAgents` 这类目录里，`enabled` 的含义只有"文件在不在启动目录里"。
/// 我们不去猜 plist 里的 `Disabled` 键，所以界面文案说的是"在启动目录中"。
fn item_from(path: &Path, source: &SourceDirs, enabled: bool, backup: bool) -> Option<StartupItem> {
    let name = path.file_name()?.to_string_lossy().to_string();
    Some(StartupItem {
        id: format!("{}-{}", source.prefix, name),
        name,
        command: path.to_string_lossy().to_string(),
        source: if backup {
            format!("{}（备份）", source.source_label())
        } else {
            source.source_label()
        },
        enabled,
        location: path
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default(),
        operable: true,
    })
}

impl SourceDirs {
    fn source_label(&self) -> String {
        match self.prefix.as_str() {
            "macos-agent" => "LaunchAgent".to_string(),
            "linux-autostart" => "XDG Autostart".to_string(),
            other => other.to_string(),
        }
    }

    /// 列出某个落点里符合后缀的普通文件；目录不递归（`out` 子目录因此自然被跳过）。
    fn entries(&self, dir: &Path, enabled: bool, backup: bool) -> Vec<StartupItem> {
        let Ok(read) = fs::read_dir(dir) else {
            // 目录不存在是常态（没禁用过任何东西），不是错误
            return Vec::new();
        };
        let mut out = Vec::new();
        for entry in read.flatten() {
            let path = entry.path();
            if !matches_extension(&path, &self.extension) {
                continue;
            }
            if !entry_is_plain_file(&path) {
                continue;
            }
            if let Some(item) = item_from(&path, self, enabled, backup) {
                out.push(item);
            }
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    /// 启动目录里的在位项。
    pub fn live_entries(&self) -> Vec<StartupItem> {
        self.entries(&self.dir, true, false)
    }

    /// 备份落点里的项：仍然列出来，标成未启用，且可以一键恢复。
    pub fn backup_entries(&self) -> Vec<StartupItem> {
        match self.backup_dir() {
            Some(dir) => self.entries(&dir, false, true),
            None => Vec::new(),
        }
    }
}

/// 把"本应用管得着的两半"（在位项 + 备份项）并进外来来源的列表，同名以在位项为准。
/// 单独开出来是为了让去重这条分支能被真的端到端测到，而不是只能靠真机上的 `~/Library/LaunchAgents`。
pub fn merge(layout: &Layout, mut out: Vec<StartupItem>) -> Vec<StartupItem> {
    let mut seen: std::collections::HashSet<String> = out.iter().map(|i| i.id.clone()).collect();
    for source in &layout.sources {
        for item in source.live_entries() {
            seen.insert(item.id.clone());
            out.push(item);
        }
    }
    for source in &layout.sources {
        for item in source.backup_entries() {
            if seen.insert(item.id.clone()) {
                out.push(item);
            }
        }
    }
    out
}

/// 界面看到的完整一页：不可操作的来源（Login Items / 注册表 / systemd）+ 每个可操作来源的
/// 在位项与备份项。
///
/// 同名时以在位项为准 —— 用户禁用了 `com.x.plist`、之后那个软件自己又写回一份，
/// 这时列两行会让 React rowKey 直接撞，而且"恢复"该改哪一份没有答案。宁可少列一份备份，
/// 也不能给出一个点了会说不清动的是哪个文件的按钮。
pub fn list_with_backup(backup_root: Option<&Path>, home: Option<&Path>) -> Vec<StartupItem> {
    merge(&layout_for(backup_root, home), crate::cleanup::get_startup_items())
}

fn matches_extension(path: &Path, extension: &str) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case(extension))
        == Some(true)
}

/// 只认真通规文件：符号链接、目录、fifo 都不算，避免"禁用"把一个指向别处的链接搬走。
fn entry_is_plain_file(path: &Path) -> bool {
    fs::symlink_metadata(path).map(|m| m.is_file()).unwrap_or(false)
}

fn file_size(path: &Path) -> u64 {
    fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

/// 目录必须存在、可 canonicalize，且不在黑名单里。返回 canonical 形式供后续前缀比较。
fn canonical_dir(dir: &Path, what: &str) -> CommandResult<PathBuf> {
    let canonical = canonicalize_clean(dir).map_err(|_| {
        AppError::not_found(format!("{}不存在或不可访问: {}", what, sanitize(&dir.to_string_lossy())))
    })?;
    if is_denied(&canonical) {
        return Err(AppError::path_denied(format!("{}位于受保护位置", what)).with_detail(sanitize(&canonical.to_string_lossy())));
    }
    Ok(canonical)
}

/// 落点目录允许还不存在（第一次禁用时才建 `backup/<prefix>`）。
/// 这时不能直接把原始路径当成落点：`..` 与符号链接还没折回真实位置。
/// 逐级往上找到能 canonicalize 的那一层，再把缺的组件原样接回去。
fn canonical_target_dir(dir: &Path) -> CommandResult<PathBuf> {
    let mut missing: Vec<&OsStr> = Vec::new();
    let mut cursor = dir;
    loop {
        if let Ok(base) = canonicalize_clean(cursor) {
            let mut base = base;
            for name in missing.iter().rev() {
                base.push(name);
            }
            return Ok(base);
        }
        let (parent, name) = match (cursor.parent(), cursor.file_name()) {
            (Some(parent), Some(name)) => (parent, name),
            _ => return Err(AppError::failed("无法确定启动项备份目录的位置")),
        };
        missing.push(name);
        // 备份根之下最多两层（`<prefix>/out`），更深说明拼错了路径
        if missing.len() > 4 {
            return Err(AppError::failed("启动项备份目录层级过深，已放弃"));
        }
        cursor = parent;
    }
}

/// 一个已经解析完的目标：叫什么、此刻真身在哪、要去哪。
struct Resolved {
    file_name: String,
    from: PathBuf,
    to: PathBuf,
}

/// 把 `id` 认成某落点里的一枚普通文件，并算出它要去的落点。
///
/// `id` 的后半截必须**就是一枚文件名**：不含分隔符、不是 `.`/`..`、后缀对得上。
/// 再叠一层 canonical 比较 —— 链接指向目录外时（例如 `~/Library/LaunchAgents/evil.plist` →
/// `/System/Library/LaunchAgents/x.plist`）两步结果不同，当场拒掉。
fn resolve(source: &SourceDirs, id: &str, from_dir: &Path, to_dir: &Path) -> CommandResult<Resolved> {
    let head = format!("{}-", source.prefix);
    let rest = id.strip_prefix(&head).ok_or_else(|| {
        AppError::invalid_input(format!("启动项 id 应以 {head} 开头，收到的是另一项"))
    })?;
    if rest.is_empty()
        || rest == "."
        || rest == ".."
        || rest.contains('/')
        || rest.contains('\\')
        || rest.contains('\0')
    {
        return Err(AppError::path_denied("启动项 id 里不允许出现路径分隔符"));
    }
    if !matches_extension(Path::new(rest), &source.extension) {
        return Err(AppError::invalid_input(format!(
            "{} 启动项必须是一枚 .{} 文件",
            source.source_label(),
            source.extension
        )));
    }
    // canonical 之后还得正好躺在同一层：`Path::new(rest).file_name()` 已经挡掉了分隔符，
    // 这一步挡的是符号链接指向外部。
    let canonical_from_dir = canonical_dir(from_dir, "启动项所在目录")?;
    let candidate = canonical_from_dir.join(rest);
    let canonical_file = canonicalize_clean(&candidate).map_err(|_| {
        AppError::not_found(format!("启动项文件不存在: {}", sanitize(&candidate.to_string_lossy())))
    })?;
    if canonical_file.parent().map(|p| p.as_os_str()) != Some(canonical_from_dir.as_os_str()) {
        return Err(AppError::path_denied("启动项不在允许的启动目录里").with_detail(sanitize(&canonical_file.to_string_lossy())));
    }
    if is_denied(&canonical_file) {
        return Err(AppError::path_denied("启动项位于受保护位置"));
    }
    // 普通文件判定要作用在**未解析**的那条路径上。`canonicalize` 会跟随链接取真身，
    // 于是"指向同目录另一份 plist 的链接"在这里看成合法普通文件，搬走的是邻居的真身、
    // 原地留下一个断链，而且 `from` 与 `file_name` 各说一套（dotfiles 仓库常这么摆 plist）。
    // 放在 canonicalize 之后是为了让"根本不存在"仍然报 NOT_FOUND 而不是"不是普通文件"。
    if !entry_is_plain_file(&candidate) {
        return Err(AppError::invalid_input("选中的启动项不是普通文件，不予移动"));
    }
    let bytes = file_size(&canonical_file);
    if bytes > MAX_STARTUP_FILE_BYTES {
        return Err(AppError::invalid_input(format!(
            "文件 {} B 超过 {} B 上限，不像是一份启动项",
            bytes, MAX_STARTUP_FILE_BYTES
        )));
    }

    let canonical_to_dir = canonical_target_dir(to_dir)?;
    let to = canonical_to_dir.join(rest);
    if to == canonical_file {
        return Err(AppError::invalid_input("来源与落点是同一个文件"));
    }
    if to.starts_with(&canonical_file) {
        return Err(AppError::invalid_input("落点落在被移动文件之下"));
    }
    Ok(Resolved {
        file_name: rest.to_string(),
        from: canonical_file,
        to,
    })
}

/// 找到 `id` 属于哪个可操作来源；找不到就说明这一项后端动不了。
fn source_for<'a>(layout: &'a Layout, id: &str, action: StartupAction) -> CommandResult<&'a SourceDirs> {
    if let Some(source) = layout
        .sources
        .iter()
        .find(|s| id.starts_with(&format!("{}-", s.prefix)))
    {
        return Ok(source);
    }
    let known: Vec<&str> = layout
        .sources
        .iter()
        .map(|s| s.prefix.as_str())
        .collect();
    let scope = if known.is_empty() {
        "这台机器上没有可由本应用安全改写的启动项".to_string()
    } else {
        format!("本应用只能操作这些来源：{}", known.join("、"))
    };
    Err(AppError::unsupported(format!(
        "{}启动项暂不支持（id 前缀不在白名单里）。{scope}",
        action.label()
    )))
}

fn require_confirm(name: &str, confirm_name: &str, action: &str) -> CommandResult<()> {
    if confirm_name == name {
        return Ok(());
    }
    Err(AppError::invalid_input(format!(
        "确认文本与要{action}的启动项名称不完全一致，已取消操作"
    )))
}

/// 真正的搬运动作。调用前 `resolve` 已经把两头都校验过了。
fn hand_over(resolved: &Resolved, to_dir: &Path) -> CommandResult<u64> {
    let bytes = file_size(&resolved.from);
    if !to_dir.exists() {
        // 第一次禁用才会走到这里：备份目录是懒创建的（`page()` 只读不建）
        fs::create_dir_all(to_dir).map_err(|e| {
            AppError::failed(format!("创建备份目录失败: {}", e)).with_detail(sanitize(&to_dir.to_string_lossy()))
        })?;
    }
    if resolved.to.exists() {
        // 这里必须是拒绝而不是 rename：unix 的 rename 会静默顶掉同名文件，丢的是上一份备份。
        return Err(AppError::invalid_input(format!(
            "目标位置已有同名文件，已取消（不覆盖既有备份）: {}",
            sanitize(&resolved.to.to_string_lossy())
        )));
    }
    fs::rename(&resolved.from, &resolved.to).map_err(|e| {
        AppError::failed(format!("移动启动项失败: {}", e)).with_detail(format!(
            "{} -> {}",
            sanitize(&resolved.from.to_string_lossy()),
            sanitize(&resolved.to.to_string_lossy())
        ))
    })?;
    Ok(bytes)
}

/// 一次动作的落点选择：禁用/删除去备份目录（删除多一层 `out`），恢复回启动目录。
pub fn operate(layout: &Layout, action: StartupAction, id: &str, confirm_name: &str) -> CommandResult<StartupOutcome> {
    let source = source_for(layout, id, action)?;
    let (from_dir, to_dir) = match action {
        StartupAction::Disable => (source.dir.clone(), source.backup_dir().ok_or_else(no_backup_root)?),
        StartupAction::Remove => (source.dir.clone(), source.out_dir().ok_or_else(no_backup_root)?),
        StartupAction::Restore => (source.backup_dir().ok_or_else(no_backup_root)?, source.dir.clone()),
    };
    let resolved = resolve(source, id, &from_dir, &to_dir)?;
    require_confirm(&resolved.file_name, confirm_name, action.label())?;

    let bytes = hand_over(&resolved, &to_dir)?;
    let listed = match action {
        StartupAction::Disable | StartupAction::Restore => true,
        // 删除后不再列进界面；文件仍在 out 目录，界面上会把这条路径原样说给用户
        StartupAction::Remove => false,
    };
    let message = match action {
        StartupAction::Disable => format!(
            "已禁用：原件移入备份目录，下次登录不再加载。恢复请看列表里的同名项。备份位置 {}",
            sanitize(&resolved.to.to_string_lossy())
        ),
        StartupAction::Remove => format!(
            "已删除：已从启动项列表移出，本应用不做物理删除。备份位置 {}",
            sanitize(&resolved.to.to_string_lossy())
        ),
        StartupAction::Restore => format!(
            "已恢复：文件移回启动目录 {}，下次登录会重新加载",
            sanitize(&resolved.to.to_string_lossy())
        ),
    };

    #[cfg(debug_assertions)]
    eprintln!(
        "[startup] {} {} -> {}",
        sanitize(&resolved.from.to_string_lossy()),
        action.label(),
        sanitize(&resolved.to.to_string_lossy())
    );

    Ok(StartupOutcome {
        id: id.to_string(),
        name: resolved.file_name.clone(),
        action,
        from_path: resolved.from.to_string_lossy().to_string(),
        to_path: resolved.to.to_string_lossy().to_string(),
        bytes,
        listed,
        message,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    fn fixture(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "zsys-startup-{}-{}-{}",
            std::process::id(),
            tag,
            SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("临时目录应可建");
        dir
    }

    /// 测试自己拼布局：用 `macos-agent` 这套形状，但两个目录都指向临时目录，
    /// 这样同一套断言在 Linux/Windows CI 上也在验移动逻辑本身（平台表另有单独的测试钉住）。
    fn test_layout(home: &Path, backup_root: &Path) -> Layout {
        build_layout(
            vec![(
                "macos-agent".to_string(),
                home.join("Library/LaunchAgents"),
                "plist".to_string(),
            )],
            Some(backup_root),
        )
    }

    struct Arena {
        home: PathBuf,
        backup: PathBuf,
        agents: PathBuf,
        layout: Layout,
    }

    fn arena(tag: &str) -> Arena {
        let home = fixture(tag);
        let backup = fixture(&format!("{tag}-backup"));
        let agents = home.join("Library/LaunchAgents");
        fs::create_dir_all(&agents).unwrap();
        let layout = test_layout(&home, &backup);
        Arena { home, backup, agents, layout }
    }

    fn put(a: &Arena, name: &str, body: &str) -> PathBuf {
        let path = a.agents.join(name);
        fs::write(&path, body).unwrap();
        path
    }

    fn id_of(name: &str) -> String {
        format!("macos-agent-{name}")
    }

    /// 本模块自己的源码（有几条接线/门禁断言要看它本身）
    fn source_of_self() -> String {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        fs::read_to_string(manifest.join("src/startup.rs")).unwrap()
    }

    /// 备份目录里现存的项（生产侧由 `merge` 拼进列表，测试就从它派生）
    fn backup_items(layout: &Layout) -> Vec<StartupItem> {
        merge(layout, Vec::new())
            .into_iter()
            .filter(|i| !i.enabled)
            .collect()
    }

    /// 一枚"本应用动不了"的外来项（Login Items 那类），用来验合并那一步不挑食。
    fn startup_item_for(id: &str, name: &str) -> StartupItem {
        StartupItem {
            id: id.to_string(),
            name: name.to_string(),
            command: name.to_string(),
            source: "Login Items".to_string(),
            enabled: true,
            location: String::new(),
            operable: false,
        }
    }

    /// 禁用 = 整份移出启动目录、落进备份目录，内容一个字节都不差。
    #[test]
    fn disabling_moves_the_file_into_the_backup_dir_byte_for_byte() {
        let a = arena("disable");
        let src = put(&a, "com.example.agent.plist", "<plist>payload-42</plist>");
        // 动之前先记下 canonical 真身：rename 之后再 canonicalize 只会拿到 Err，
        // 那时断言写成"失败就用返回值兜底"等于什么都没验。
        // 用 canonicalize_clean 而不是 fs::canonicalize：后者在 Windows 上给出 `\\?\C:\...`
        // 这种 verbatim 形式，而产品侧回给用户的两条路径一律过 strip_verbatim（见 cleanup.rs 的
        // 那条注释：verbatim 会绕过按字符串前缀判定的黑名单）。拿裸 canonicalize 当预期，
        // 测的就不是"回显口径"而是"这台机器的 Windows 路径表示法"了。
        let src_canonical = canonicalize_clean(&src).unwrap();
        let outcome = operate(&a.layout, StartupAction::Disable, &id_of("com.example.agent.plist"), "com.example.agent.plist")
            .expect("禁用应成功");

        assert!(!src.exists(), "原件必须已经离开启动目录，否则下次登录照样加载");
        let landed = a.backup.join("macos-agent").join("com.example.agent.plist");
        assert!(landed.exists(), "备份目录里应能看到它：{outcome:?}");
        assert_eq!(fs::read_to_string(&landed).unwrap(), "<plist>payload-42</plist>");
        assert_eq!(outcome.bytes, 25, "字节数应与源文件一致：<plist>payload-42</plist> 是 25 B：{outcome:?}");
        // 回给用户的两头路径都是 canonical 真身（macOS 的 /var 会折成 /private/var）
        assert_eq!(outcome.to_path, canonicalize_clean(&landed).unwrap().to_string_lossy());
        assert_eq!(outcome.from_path, src_canonical.to_string_lossy(), "from_path 要能对着访达找得到");
        assert!(outcome.listed, "禁用项仍然列在界面里才能恢复");
        assert_eq!(outcome.action, StartupAction::Disable);
        assert!(outcome.message.contains("下次登录不再加载"), "{}", outcome.message);
        // 备份目录里只有这一枚，没有半成的临时文件
        assert_eq!(fs::read_dir(a.backup.join("macos-agent")).unwrap().count(), 1);
        let _ = fs::remove_dir_all(&a.home);
        let _ = fs::remove_dir_all(&a.backup);
    }

    /// 禁用后列表里仍看得见这一项，且状态是"未启用"、来源标着备份 —— 界面据此给"恢复"按钮。
    #[test]
    fn a_disabled_item_stays_visible_in_the_list_and_says_it_is_a_backup() {
        let a = arena("list");
        put(&a, "com.example.keep.plist", "x");
        put(&a, "com.example.off.plist", "y");
        assert_eq!(a.layout.sources[0].live_entries().len(), 2);

        operate(&a.layout, StartupAction::Disable, &id_of("com.example.off.plist"), "com.example.off.plist").unwrap();
        let backup = backup_items(&a.layout);
        assert_eq!(backup.len(), 1, "{backup:?}");
        let item = &backup[0];
        assert_eq!(item.id, id_of("com.example.off.plist"));
        assert!(!item.enabled, "备份里的项不能报成已启用");
        assert!(item.source.contains("备份"), "来源要说明这是备份：{}", item.source);
        assert!(item.operable, "备份项要能被恢复");
        assert_eq!(item.location, a.backup.join("macos-agent").to_string_lossy());
        // `out` 子目录不该被列成备份项
        fs::create_dir_all(a.backup.join("macos-agent").join("out")).unwrap();
        fs::write(a.backup.join("macos-agent").join("out").join("com.example.gone.plist"), "z").unwrap();
        assert_eq!(backup_items(&a.layout).len(), 1, "删除掉的项不该再出现在列表里");
        let _ = fs::remove_dir_all(&a.home);
        let _ = fs::remove_dir_all(&a.backup);
    }

    /// 同名只列一行，且以在位项为准：两行会让 rowKey 撞，"恢复"也说不清动的是哪一份。
    #[test]
    fn a_backup_copy_is_hidden_while_an_item_with_the_same_name_is_live_again() {
        let a = arena("dedupe");
        let name = "com.example.twicehome.plist";
        let backup_dir = a.backup.join("macos-agent");
        fs::create_dir_all(&backup_dir).unwrap();
        // 备份里有一份旧的（当初禁用留下），启动目录里软件又写回了一份
        fs::write(backup_dir.join(name), "OLD-BACKUP").unwrap();
        put(&a, name, "NEW-LIVE");

        let merged = merge(&a.layout, Vec::new());
        let rows: Vec<&StartupItem> = merged.iter().filter(|i| i.id == id_of(name)).collect();
        assert_eq!(rows.len(), 1, "同名只能有一行：{merged:?}");
        assert!(rows[0].enabled, "在位项优先，不能被备份行顶掉状态");
        assert_eq!(rows[0].command, a.agents.join(name).to_string_lossy());
        // 旧的备份副本还在原地，没被列出来也没被动过
        assert_eq!(fs::read_to_string(backup_dir.join(name)).unwrap(), "OLD-BACKUP");
        // 外来来源（Login Items 那类）原样带过，且不会把可操作项挤掉
        let foreign = vec![startup_item_for("macos-login-0", "SomeApp")];
        let merged = merge(&a.layout, foreign);
        assert!(merged.iter().any(|i| i.id == "macos-login-0"));
        assert_eq!(merged.iter().filter(|i| i.id == id_of(name)).count(), 1);
        assert!(!merged.iter().find(|i| i.id == "macos-login-0").unwrap().operable);
        let _ = fs::remove_dir_all(&a.home);
        let _ = fs::remove_dir_all(&a.backup);
    }

    /// 恢复把文件放回启动目录，备份目录清空。
    #[test]
    fn restoring_puts_the_file_back_into_the_startup_dir() {
        let a = arena("restore");
        let src = put(&a, "com.example.back.plist", "body");
        operate(&a.layout, StartupAction::Disable, &id_of("com.example.back.plist"), "com.example.back.plist").unwrap();
        assert!(!src.exists());

        let outcome = operate(&a.layout, StartupAction::Restore, &id_of("com.example.back.plist"), "com.example.back.plist")
            .expect("恢复应成功");
        assert!(src.exists(), "文件要回到原来的启动目录");
        assert_eq!(fs::read_to_string(&src).unwrap(), "body");
        assert!(!a.backup.join("macos-agent").join("com.example.back.plist").exists());
        assert!(outcome.listed);
        assert!(outcome.message.contains("下次登录会重新加载"), "{}", outcome.message);
        assert!(backup_items(&a.layout).is_empty());
        let _ = fs::remove_dir_all(&a.home);
        let _ = fs::remove_dir_all(&a.backup);
    }

    /// 删除：不列进界面，但**文件仍在** —— 本应用没有一处 `remove_file`。
    #[test]
    fn removing_drops_it_from_the_list_without_ever_deleting_the_bytes() {
        let a = arena("remove");
        let src = put(&a, "com.example.gone.plist", "still-here");
        let outcome = operate(&a.layout, StartupAction::Remove, &id_of("com.example.gone.plist"), "com.example.gone.plist")
            .expect("删除应成功");

        assert!(!src.exists());
        assert!(!outcome.listed, "删除后不再列进界面");
        let kept = a.backup.join("macos-agent").join("out").join("com.example.gone.plist");
        assert!(kept.exists(), "必须留一份备份，删除不能等于抹掉：{outcome:?}");
        assert_eq!(fs::read_to_string(&kept).unwrap(), "still-here");
        assert!(backup_items(&a.layout).is_empty(), "删除项不出现在备份列表里");
        assert!(outcome.message.contains("不做物理删除"), "{}", outcome.message);
        assert!(outcome.message.contains("备份位置"), "得告诉用户备份在哪：{}", outcome.message);
        let _ = fs::remove_dir_all(&a.home);
        let _ = fs::remove_dir_all(&a.backup);
    }

    /// 确认闸门：名称差一个字符就不动，且什么都没移动。
    #[test]
    fn an_inexact_confirmation_name_stops_the_operation_before_any_move() {
        let a = arena("confirm");
        let src = put(&a, "com.example.gate.plist", "keep-me");
        let id = id_of("com.example.gate.plist");

        // "Com.Example.Gate.plist" 与真名只差大小写：少了这一条，比较写成
        // `eq_ignore_ascii_case` 也能骗过整段测试（第一轮变异就是这么逃逸的）。
        // 后面那串全大写是 `PLLIST`（多一个 L），它测的是"长度不同也要拒"。
        for wrong in [
            "",
            "com.example.gate.plis",
            "com.example.gate.plist ",
            "Com.Example.Gate.plist",
            "COM.EXAMPLE.GATE.PLLIST",
            "com.example.gate.plist\n",
        ] {
            let err = operate(&a.layout, StartupAction::Disable, &id, wrong).unwrap_err();
            assert_eq!(err.code, "INVALID_INPUT", "确认文本 {wrong:?} 应被拒：{err:?}");
            assert!(err.message.contains("不完全一致"), "{}", err.message);
            assert!(src.exists(), "被拒之后文件不许离开原位");
            assert!(backup_items(&a.layout).is_empty());
        }
        // 逐字对上才放行（这条断言同时防止"确认比较其实写反了"）
        assert!(operate(&a.layout, StartupAction::Disable, &id, "com.example.gate.plist").is_ok());
        assert!(!src.exists());
        let _ = fs::remove_dir_all(&a.home);
        let _ = fs::remove_dir_all(&a.backup);
    }

    /// id 里塞路径分隔符 / `..` 一律拒：这条命令不接受渲染进程提交路径。
    #[test]
    fn an_id_carrying_any_path_separator_or_dotdot_is_refused() {
        let a = arena("traverse");
        // 先放一枚真文件，保证"被拒"不是因为目录里没有这个后缀
        put(&a, "com.example.real.plist", "x");
        let outside = a.home.join("outside.plist");
        fs::write(&outside, "do-not-touch").unwrap();

        let cases = [
            "macos-agent-../../outside.plist",
            "macos-agent-../com.example.real.plist",
            "macos-agent-./com.example.real.plist",
            "macos-agent-/Users/Shared/x.plist",
            "macos-agent-sub/com.example.real.plist",
            "macos-agent-..",
            "macos-agent-.",
            "macos-agent-sub\\..\\outside.plist",
            "macos-agent-\u{0}x.plist",
        ];
        for id in cases {
            let err = operate(&a.layout, StartupAction::Remove, id, "x.plist").unwrap_err();
            assert!(
                err.code == "PATH_DENIED" || err.code == "NOT_FOUND" || err.code == "INVALID_INPUT",
                "{id:?} 应被路径校验拦下，实际 {err:?}"
            );
            assert_eq!(err.code, "PATH_DENIED", "{id:?} 应当按路径拒绝而不是别的错：{err:?}");
        }
        assert_eq!(fs::read_to_string(&outside).unwrap(), "do-not-touch");
        assert!(a.agents.join("com.example.real.plist").exists());
        let _ = fs::remove_dir_all(&a.home);
        let _ = fs::remove_dir_all(&a.backup);
    }

    /// 白名单之外的来源直接 UNSUPPORTED，并且不去猜路径。
    #[test]
    fn only_the_whitelisted_id_prefixes_are_even_attempted() {
        let a = arena("prefix");
        put(&a, "com.example.ok.plist", "x");
        for id in [
            "macos-login-0",
            "win-run-SecurityHealth",
            "linux-systemd-foo.service",
            "linux-autostart-x.desktop",
            "",
            "macos-agent",
            "macos-agentX-com.example.ok.plist",
        ] {
            let err = operate(&a.layout, StartupAction::Disable, id, "com.example.ok.plist").unwrap_err();
            assert_eq!(err.code, "UNSUPPORTED", "{id:?} 不在白名单里，应明确说不支持：{err:?}");
            assert!(err.message.contains("本应用只能操作"), "错误要交代边界：{}", err.message);
        }
        assert!(a.agents.join("com.example.ok.plist").exists());
        // 布局里一个来源都没有时（Windows 就是这样），任何 id 都只能拿到 UNSUPPORTED
        let empty = Layout::default();
        let err = operate(&empty, StartupAction::Remove, "macos-agent-x.plist", "x.plist").unwrap_err();
        assert_eq!(err.code, "UNSUPPORTED");
        assert!(err.message.contains("没有可由本应用安全改写的启动项"), "{}", err.message);
        let _ = fs::remove_dir_all(&a.home);
        let _ = fs::remove_dir_all(&a.backup);
    }

    /// 后缀与体积：不是 .plist、或者大得不像启动项，都不许动。
    #[test]
    fn a_wrong_extension_or_an_implausibly_large_file_is_not_moved() {
        let a = arena("shape");
        let txt = put(&a, "notes.txt", "not a plist");
        let err = operate(&a.layout, StartupAction::Disable, "macos-agent-notes.txt", "notes.txt").unwrap_err();
        assert_eq!(err.code, "INVALID_INPUT", "{err:?}");
        assert!(err.message.contains(".plist"), "要说清要求哪个后缀：{}", err.message);
        assert!(txt.exists());

        // 后缀对、体积离谱（例如有人把归档改名成 .plist）：拒绝，并给出上下限数字
        let big_name = "com.example.big.plist";
        let big = put(&a, big_name, "");
        fs::write(&big, vec![b'x'; (MAX_STARTUP_FILE_BYTES + 1) as usize]).unwrap();
        let err = operate(&a.layout, StartupAction::Disable, &id_of(big_name), big_name).unwrap_err();
        assert_eq!(err.code, "INVALID_INPUT", "{err:?}");
        assert!(err.message.contains("超过"), "{}", err.message);
        assert!(err.message.contains(&MAX_STARTUP_FILE_BYTES.to_string()), "得报出真实上限：{}", err.message);
        assert!(big.exists());

        // 上限之内照样放行，证明上面那条不是因为"任何文件都不许动"
        let ok = put(&a, "com.example.small.plist", "x");
        assert!(operate(&a.layout, StartupAction::Disable, &id_of("com.example.small.plist"), "com.example.small.plist").is_ok());
        assert!(!ok.exists());
        let _ = fs::remove_dir_all(&a.home);
        let _ = fs::remove_dir_all(&a.backup);
    }

    /// 不覆盖：目标位已有同名文件时必须拒绝 —— unix 的 `rename` 会静默顶掉它，丢的是上一份备份。
    #[test]
    fn an_occupied_backup_slot_is_refused_instead_of_overwriting_the_earlier_copy() {
        let a = arena("overwrite");
        let backup_dir = a.backup.join("macos-agent");
        fs::create_dir_all(&backup_dir).unwrap();
        let name = "com.example.dup.plist";
        // 备份里已经有一份"上一次禁用留下的原件"，内容不同
        fs::write(backup_dir.join(name), "PREVIOUS-BACKUP").unwrap();
        let live = put(&a, name, "LIVE-COPY");

        let err = operate(&a.layout, StartupAction::Disable, &id_of(name), name).unwrap_err();
        assert_eq!(err.code, "INVALID_INPUT", "同名备份占位必须拒：{err:?}");
        assert!(err.message.contains("不覆盖既有备份"), "{}", err.message);
        assert_eq!(fs::read_to_string(backup_dir.join(name)).unwrap(), "PREVIOUS-BACKUP", "上一份备份被顶掉了");
        assert!(live.exists(), "拒了就不该动源文件");

        // 恢复方向同理：启动目录里已经有同名文件（软件自己写回来的那份）时不许盖
        let err = operate(&a.layout, StartupAction::Restore, &id_of(name), name).unwrap_err();
        assert_eq!(err.code, "INVALID_INPUT", "恢复撞上同名在位文件必须拒：{err:?}");
        assert!(err.message.contains("不覆盖既有"), "{}", err.message);
        assert_eq!(fs::read_to_string(a.agents.join(name)).unwrap(), "LIVE-COPY", "在位那份被恢复流程盖掉了");
        assert_eq!(fs::read_to_string(backup_dir.join(name)).unwrap(), "PREVIOUS-BACKUP");
        // 备份目录空出来之后才谈得上恢复
        fs::remove_file(backup_dir.join(name)).unwrap();
        fs::write(backup_dir.join(name), "x").unwrap();
        let err = operate(&a.layout, StartupAction::Restore, &id_of(name), name).unwrap_err();
        assert_eq!(err.code, "INVALID_INPUT", "在位文件还在，恢复依旧该拒：{err:?}");
        let _ = fs::remove_dir_all(&a.home);
        let _ = fs::remove_dir_all(&a.backup);
    }

    /// 文件不存在时要说 NOT_FOUND，而不是"成功但什么都没做"。
    #[test]
    fn a_missing_item_reports_not_found_rather_than_a_quiet_success() {
        let a = arena("missing");
        fs::create_dir_all(a.agents.join("nested")).unwrap();
        let err = operate(&a.layout, StartupAction::Disable, &id_of("com.example.ghost.plist"), "com.example.ghost.plist")
            .unwrap_err();
        assert_eq!(err.code, "NOT_FOUND", "{err:?}");
        assert!(err.message.contains("不存在"), "{}", err.message);
        // 禁用第二次会走到这里：第一次成功，第二次原件已不在启动目录
        put(&a, "com.example.twice.plist", "x");
        operate(&a.layout, StartupAction::Disable, &id_of("com.example.twice.plist"), "com.example.twice.plist").unwrap();
        let err = operate(&a.layout, StartupAction::Disable, &id_of("com.example.twice.plist"), "com.example.twice.plist")
            .unwrap_err();
        assert_eq!(err.code, "NOT_FOUND", "重复禁用不该报成功：{err:?}");
        assert_eq!(backup_items(&a.layout).len(), 1, "也不该留下第二份");
        let _ = fs::remove_dir_all(&a.home);
        let _ = fs::remove_dir_all(&a.backup);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_inside_the_startup_dir_pointing_elsewhere_is_refused() {
        let a = arena("symlink");
        let victim = a.home.join("precious.plist");
        fs::write(&victim, "DO-NOT-MOVE").unwrap();
        std::os::unix::fs::symlink(&victim, a.agents.join("com.example.link.plist")).unwrap();

        let err = operate(&a.layout, StartupAction::Remove, &id_of("com.example.link.plist"), "com.example.link.plist")
            .unwrap_err();
        assert!(
            err.code == "INVALID_INPUT" || err.code == "PATH_DENIED",
            "链接应当被拒，实际 {err:?}"
        );
        assert_eq!(fs::read_to_string(&victim).unwrap(), "DO-NOT-MOVE", "链接指向的真身被搬走了");
        assert!(a.agents.join("com.example.link.plist").symlink_metadata().is_ok(), "链接本身也该留在原地");
        let _ = fs::remove_dir_all(&a.home);
        let _ = fs::remove_dir_all(&a.backup);
    }

    /// 指向**同一目录内**另一份启动项的链接：canonical 后的父目录仍然合法，
    /// 所以拦住它的只能是"必须是一枚普通文件"这一道。上一轮的用例指向目录外，
    /// 两道闸叠在一起，把普通文件判定改成跟随符号链接也测不出来（第二轮变异就这么逃逸的）。
    #[cfg(unix)]
    #[test]
    fn a_symlink_pointing_at_a_neighbour_in_the_same_dir_is_still_not_a_startup_item() {
        let a = arena("link-in-dir");
        let real = put(&a, "com.example.real.plist", "REAL-BODY");
        let link = a.agents.join("com.example.alias.plist");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let err =
            operate(&a.layout, StartupAction::Disable, &id_of("com.example.alias.plist"), "com.example.alias.plist")
                .unwrap_err();
        assert_eq!(err.code, "INVALID_INPUT", "链接不是普通文件，只能由这一道闸拒掉：{err:?}");
        assert!(err.message.contains("普通文件"), "{}", err.message);
        // 列表侧本来就不列链接：两边口径一致，界面上不会出现这一行
        assert!(!a.layout.sources[0]
            .live_entries()
            .iter()
            .any(|i| i.id == id_of("com.example.alias.plist")), "链接不该被列成启动项");
        assert_eq!(fs::read_to_string(&real).unwrap(), "REAL-BODY", "链接指向的真身被动过");
        assert!(link.symlink_metadata().is_ok(), "链接自己也不许离开启动目录");
        assert!(backup_items(&a.layout).is_empty(), "被拒之后备份目录里不该多出东西");
        let _ = fs::remove_dir_all(&a.home);
        let _ = fs::remove_dir_all(&a.backup);
    }

    #[test]
    fn a_directory_or_non_file_entry_is_not_moved() {
        let a = arena("dir");
        // 后缀是 .plist 的目录：不能因为"在启动目录里"就被当成启动项
        fs::create_dir_all(a.agents.join("com.example.folder.plist")).unwrap();
        let err = operate(&a.layout, StartupAction::Disable, &id_of("com.example.folder.plist"), "com.example.folder.plist")
            .unwrap_err();
        assert!(err.code == "INVALID_INPUT" || err.code == "NOT_FOUND", "{err:?}");
        assert!(a.agents.join("com.example.folder.plist").is_dir());
        // 列目录时也要把它跳过，而不是显示一枚点开没内容的假项
        fs::write(a.agents.join("com.example.real.plist"), "x").unwrap();
        let live = a.layout.sources[0].live_entries();
        assert_eq!(live.len(), 1, "目录不该被列成启动项：{live:?}");
        assert_eq!(live[0].id, id_of("com.example.real.plist"));
        let _ = fs::remove_dir_all(&a.home);
        let _ = fs::remove_dir_all(&a.backup);
    }

    /// 备份根目录压根不存在时（第一次运行、从没禁用过）列目录不能报错。
    #[test]
    fn listing_with_no_backup_directory_yet_is_an_empty_page_not_an_error() {
        let home = fixture("nobackup");
        let backup = home.join("does-not-exist");
        let agents = home.join("Library/LaunchAgents");
        fs::create_dir_all(&agents).unwrap();
        fs::write(agents.join("com.example.only.plist"), "x").unwrap();
        let layout = test_layout(&home, &backup);
        assert!(backup_items(&layout).is_empty());
        assert_eq!(layout.sources[0].live_entries().len(), 1);
        // 启动目录不存在同样不该 panic（新装用户没有 LaunchAgents 目录是常态）
        let missing = test_layout(&home.join("nope"), &backup);
        assert!(missing.sources[0].live_entries().is_empty());
        let _ = fs::remove_dir_all(&home);
    }

    /// 平台表：这台机器上真正允许被操作的是哪些目录 —— 界面文案与错误信息都以此为口径。
    #[test]
    fn the_platform_table_offers_only_the_directories_we_can_reason_about() {
        let home = Path::new("/Users/tester");
        let sources = platform_sources(home);
        if cfg!(target_os = "macos") {
            assert_eq!(sources.len(), 1, "macOS 只放开 LaunchAgents：{sources:?}");
            assert_eq!(sources[0].0, "macos-agent");
            assert_eq!(sources[0].1, PathBuf::from("/Users/tester/Library/LaunchAgents"));
            assert_eq!(sources[0].2, "plist");
        } else if cfg!(target_os = "linux") {
            assert_eq!(sources.len(), 1, "linux 只放开 XDG autostart：{sources:?}");
            assert_eq!(sources[0].0, "linux-autostart");
            assert_eq!(sources[0].2, "desktop");
        } else {
            // Windows：来源表为空 ⇒ 任何 id 都只能拿到 UNSUPPORTED，界面据此不给按钮
            assert!(sources.is_empty());
        }
        // LaunchDaemons / /Library/LaunchAgents 需要 root，绝不出现在表里
        let all: String = platform_sources(home)
            .iter()
            .map(|(_, dir, _)| dir.to_string_lossy().to_string())
            .collect::<Vec<_>>()
            .join(",");
        assert!(!all.contains("LaunchDaemons"), "系统级守护进程目录不许进白名单：{all}");
        assert!(!all.starts_with("/Library"), "全局 LaunchAgents 需要提权，不在范围内");
    }

    /// 备份落点必须始终在备份根之下，且绝不与启动目录重合 —— 重合了"禁用"就是把文件原地搬原地。
    #[test]
    fn backup_slots_live_under_the_backup_root_and_never_on_top_of_the_source_dir() {
        let home = fixture("slots");
        let backup = fixture("slots-root");
        let layout = test_layout(&home, &backup);
        let source = &layout.sources[0];
        assert_eq!(source.backup_dir(), Some(backup.join("macos-agent")));
        assert_eq!(source.out_dir(), Some(backup.join("macos-agent").join("out")));
        assert!(!source.backup_dir().unwrap().starts_with(&source.dir));
        assert!(!source.dir.starts_with(source.backup_dir().unwrap()));
        assert_ne!(source.backup_dir(), source.out_dir());
        // 备份根解析不出来（`app_data_dir()` 失败）时：列不出备份项，任何移动也只能被拒
        let orphan = build_layout(
            vec![("macos-agent".to_string(), home.join("Library/LaunchAgents"), "plist".to_string())],
            None,
        );
        fs::create_dir_all(home.join("Library/LaunchAgents")).unwrap();
        fs::write(home.join("Library/LaunchAgents").join("com.example.lonely.plist"), "x").unwrap();
        assert_eq!(orphan.sources[0].live_entries().len(), 1, "在位项与备份根无关，照样列得出来");
        assert!(orphan.sources[0].backup_entries().is_empty());
        let err = operate(&orphan, StartupAction::Disable, &id_of("com.example.lonely.plist"), "com.example.lonely.plist")
            .unwrap_err();
        assert_eq!(err.code, "COMMAND_FAILED", "没有落点就不许动文件：{err:?}");
        assert!(err.message.contains("无法确定本应用的备份目录"), "{}", err.message);
        assert!(home.join("Library/LaunchAgents").join("com.example.lonely.plist").exists(), "被拒之后文件还原位");

        // 两个来源同名文件不能落在同一个落点里互相顶掉
        let two = build_layout(
            vec![
                ("macos-agent".to_string(), home.join("Library/LaunchAgents"), "plist".to_string()),
                ("linux-autostart".to_string(), home.join(".config/autostart"), "desktop".to_string()),
            ],
            Some(&backup),
        );
        assert_ne!(two.sources[0].backup_dir(), two.sources[1].backup_dir());
        let _ = fs::remove_dir_all(&home);
        let _ = fs::remove_dir_all(&backup);
    }

    /// `operable` 的判据要两侧都钉住：只测"白名单内为真"，一个"一律说真"的实现也能过，
    /// 后果是界面给出点了必被后端拒的按钮；只测"白名单外为假"则反过来会让恢复按钮消失。
    /// 这三枚不可操作 id 就是扫描器真实产出的形状（Login Items / 注册表 / systemd 用户单元）。
    #[test]
    fn the_operable_flag_and_the_mover_share_one_prefix_table() {
        for id in ["macos-login-0", "win-run-SecurityHealth", "linux-systemd-firewall-app"] {
            assert!(!id_is_operable(id), "{id} 需要提权或没有文件可移，不能给按钮");
        }
        let home = Path::new(if cfg!(target_os = "macos") { "/Users/tester" } else { "/home/tester" });
        let prefixes = platform_sources(home);
        assert_eq!(
            prefixes.is_empty(),
            !(cfg!(target_os = "macos") || cfg!(target_os = "linux")),
            "白名单外的平台就该一枚 id 都不放行：{prefixes:?}"
        );
        for (prefix, _, _) in prefixes {
            // 真能被 `operate` 移动的 id 必须被同一张表判成可操作，否则界面上的按钮会消失
            assert!(
                id_is_operable(&format!("{prefix}-com.example.x")),
                "{prefix} 在移动侧是白名单、在打标侧却被拒"
            );
        }
    }

    /// 真实扫描器给出的 id 必须能被自己的解析器认出来（除白名单外的来源外）。
    /// 只读：这一条不移动任何用户文件。
    #[test]
    fn every_operable_id_the_real_scanner_emits_resolves_back_to_a_file() {
        let items = crate::cleanup::get_startup_items();
        let home = dirs::home_dir().expect("测试环境应有家目录");
        let backup = fixture("real-scan");
        let layout = layout_for(Some(&backup), Some(&home));
        for item in items.iter().filter(|i| i.operable) {
            let source = source_for(&layout, &item.id, StartupAction::Disable)
                .unwrap_or_else(|e| panic!("{} 被标成可操作却不在白名单里：{e:?}", item.id));
            let resolved = resolve(source, &item.id, &source.dir, &source.backup_dir().unwrap())
                .unwrap_or_else(|e| panic!("{} 解析不回来：{e:?}", item.id));
            assert!(resolved.from.is_file(), "{} 不是普通文件", item.id);
            assert_eq!(resolved.file_name, item.name, "id 与 name 指向的文件名不一致：{item:?}");
        }
        // 不可操作的项必须确实拿不到路径（而不是测试没覆盖到）
        for item in items.iter().filter(|i| !i.operable) {
            let err = operate(&layout, StartupAction::Disable, &item.id, &item.name).unwrap_err();
            assert_eq!(err.code, "UNSUPPORTED", "{} 被标成不可操作却能过闸：{err:?}", item.id);
        }
        let _ = fs::remove_dir_all(&backup);
    }

    /// 载荷字段名与前端 interface 对齐（漂移的后果是界面把 undefined 当 0 显示）。
    #[test]
    fn the_outcome_payload_matches_the_frontend_interface() {
        use crate::contract_fixtures::{serialized_keys, ts_interface_keys};

        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let contract = fs::read_to_string(manifest.join("../src/ipc_contract.ts")).unwrap();
        let outcome = StartupOutcome {
            id: String::new(),
            name: String::new(),
            action: StartupAction::Disable,
            from_path: String::new(),
            to_path: String::new(),
            bytes: 0,
            listed: true,
            message: String::new(),
        };
        assert_eq!(
            serialized_keys(&serde_json::to_value(&outcome).unwrap()),
            ts_interface_keys(&contract, "StartupOutcome"),
            "StartupOutcome 的字段名与前端不一致"
        );
        assert_eq!(
            serialized_keys(&serde_json::to_value(&outcome).unwrap()).len(),
            8,
            "字段数变了要同时改两端"
        );
        // StartupItem 新增的 operable 也在校验范围内
        assert!(ts_interface_keys(&contract, "StartupItem").contains(&"operable".to_string()));
        for (command, rust) in [
            ("disableStartupItem", "disable_startup_item"),
            ("removeStartupItem", "remove_startup_item"),
            ("restoreStartupItem", "restore_startup_item"),
        ] {
            assert!(
                contract.contains(&format!("{command}: \"{rust}\"")),
                "前端未登记 {rust}"
            );
        }
        assert_eq!(
            serde_json::to_value(StartupAction::Disable).unwrap(),
            serde_json::json!("disable")
        );
        assert_eq!(serde_json::to_value(StartupAction::Remove).unwrap(), serde_json::json!("remove"));
        assert_eq!(
            serde_json::to_value(StartupAction::Restore).unwrap(),
            serde_json::json!("restore")
        );
    }

    /// 接线：模块进来、命令注册、命令转调本模块、界面不再有"尚未实现"的按钮。
    #[test]
    fn the_operations_are_wired_on_both_sides() {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let lib_rs = fs::read_to_string(manifest.join("src/lib.rs")).unwrap();
        assert!(lib_rs.contains("mod startup;"), "startup 模块没接进 lib.rs");
        for name in [
            "commands::disable_startup_item",
            "commands::remove_startup_item",
            "commands::restore_startup_item",
        ] {
            assert!(lib_rs.contains(name), "命令没注册进 invoke_handler: {name}");
        }
        let commands_rs = fs::read_to_string(manifest.join("src/commands.rs")).unwrap();
        for call in ["startup::run(", "startup::page("] {
            assert!(commands_rs.contains(call), "命令没有转调 startup 模块：{call}");
        }
        // 三条命令各自映射到哪一种动作，接错了会静默变成"点禁用却删了项"
        for action in [
            "startup::StartupAction::Disable",
            "startup::StartupAction::Remove",
            "startup::StartupAction::Restore",
        ] {
            assert!(commands_rs.contains(action), "命令里找不到 {action}");
        }
        for call in ["operate(&layout", "list_with_backup(", "layout_for("] {
            assert!(source_of_self().contains(call), "startup 模块内部缺一环：{call}");
        }
        assert!(
            !commands_rs.contains("pub fn get_startup_items_cmd() -> Vec<StartupItem>"),
            "扫描命令还停在无 AppHandle 的旧签名，备份里的禁用项列不出来"
        );
        let tab = fs::read_to_string(manifest.join("../src/components/tabs/StartupTab.tsx")).unwrap();
        assert!(!tab.contains("尚未实现"), "界面上还挂着\u{201c}尚未实现\u{201d}的按钮");
        // 原来那对按钮是写死的永久禁用态；现在只许在"已经有操作在跑"时短暂禁用
        assert!(!tab.contains("<Button size=\"small\" disabled>"), "按钮不该再是永久禁用态");
        assert!(tab.contains("operable"), "给不给按钮要按后端回的 operable 决定");
        assert!(tab.contains("确认"), "三种动作都要过逐字确认那道闸");
        for hook in ["onDisable", "onRemove", "onRestore"] {
            assert!(tab.contains(hook), "StartupTab 缺 {hook}");
        }
        let app = fs::read_to_string(manifest.join("../src/App.tsx")).unwrap();
        for command in [
            "Commands.disableStartupItem",
            "Commands.removeStartupItem",
            "Commands.restoreStartupItem",
        ] {
            assert!(app.contains(command), "App 没接 {command}");
        }
    }

    /// 本模块不许出现任何"物理删除启动项文件"的调用。
    #[test]
    fn the_module_never_deletes_a_startup_file_it_was_asked_to_move() {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let source = fs::read_to_string(manifest.join("src/startup.rs")).unwrap();
        // 只看生产段：测试夹具自己会清临时目录，那不算"删用户的启动项"
        let production: String = source
            .split("#[cfg(test)]")
            .next()
            .unwrap_or("")
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(production.len() > 2_000, "生产段没读到，这条门禁就成了空扫：{} B", production.len());
        for forbidden in ["fs::remove_file", "fs::remove_dir(", "std::fs::remove_file", "std::fs::remove_dir"] {
            assert!(!production.contains(forbidden), "生产代码里出现了 {forbidden}：删除只能是移动");
        }
        assert!(production.contains("fs::rename"), "移动必须走 rename");
        assert!(!production.contains("std::process::Command"), "不得借道 shell 改启动项");
        assert!(!production.contains("osascript"), "Login Items 那条通路要自动化授权，没测过就不上");
    }
}
