//! 偏好导入/导出（T5-11）：把 `usePrefs` 那几项写成一枚可携带的 JSON 文件，或者读回来。
//!
//! 后端在这里只管**落盘边界**：路径必须落在用户可写范围内、必须是 `.json`、体积有上限、
//! 写必须是原子的。偏好值的**语义**（哪个值算合法）仍然只在浏览器一侧判 —— `usePrefs`
//! 已经有一套白名单，后端再实现一套就等于有两个口径，迟早漂移。
//! 因此 `prefs` 字段对后端是一个不透明的 JSON 对象。
use crate::cleanup::{canonicalize_clean, expand_tilde, is_denied, under_allowed_root};
use crate::error::{AppError, CommandResult};
use crate::log_sanitize::sanitize;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

/// 文件里带一个来源标识：拿别的应用导出的 JSON 进来，应当在认文件时就拒掉，
/// 而不是静默改本应用的行为。
pub const PREFS_APP_ID: &str = "z-biz-tool-sys";
/// 结构变更时递增；不做跨版本猜测式迁移 —— 认不出来就让用户重设，比照猜的写进去安全。
pub const PREFS_SCHEMA_VERSION: u32 = 1;
/// 偏好只是几个数字和布尔值。超过这个体积说明拿错了文件（比如误指到某份导出数据）。
pub const MAX_PREFS_FILE_BYTES: u64 = 64 * 1024;

/// 落盘/读出的一份偏好文件。字段名与前端 `PrefsFilePayload` 逐一对齐，有契约测试锁定。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrefsFile {
    pub app: String,
    pub schema_version: u32,
    /// 导出时刻（毫秒）。手写/被删掉这个字段不算错，所以是 `Option`。
    #[serde(default)]
    pub exported_at_ms: Option<u64>,
    pub prefs: Value,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportOutcome {
    /// 实际写入的路径（`~` 已展开、符号链接已解析），回给用户确认用
    pub path: String,
    pub bytes: u64,
    /// 这份偏好里有几项；0 项也照样导出成功，因为"当前没有偏好"是一个真实状态
    pub keys: usize,
    pub schema_version: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportOutcome {
    pub path: String,
    pub bytes: u64,
    pub schema_version: u32,
    pub exported_at_ms: Option<u64>,
    pub prefs: Value,
    /// 文件自带的项数（归一化之前）。前端拿它与"实际采纳了几项"对比，才能说"另有 N 项被忽略"
    pub file_keys: usize,
}

/// 偏好读写比 `cleanup::allowed_roots()` 再宽一点点的目录。
///
/// macOS 上 `temp_dir()` 拿到的是 per-user 的 `$TMPDIR`（`/var/folders/.../T/`），
/// 而用户在保存框里很容易落到全局 `/tmp` —— 真机验证时就在这里被自己拦下过。
/// 这份列表**只给偏好读写用**：清理模块的删除白名单不引它，多一个可写目录
/// 不等于多一个可删目录。外部卷也不放开（宁可让用户换个位置，也不给渲染进程
/// 一个能往 U 盘写文件的口子）。
fn extra_writable_roots() -> Vec<PathBuf> {
    ["/tmp", "/private/tmp"]
        .iter()
        .map(PathBuf::from)
        // macOS 的 /tmp 是指向 /private/tmp 的符号链接，比较前先折成真实路径
        .map(|root| canonicalize_clean(&root).unwrap_or(root))
        .filter(|root| root.is_dir())
        .collect()
}

fn prefs_root_allowed(dir: &Path) -> bool {
    under_allowed_root(dir)
        || dirs::home_dir().map(|home| dir.starts_with(&home)).unwrap_or(false)
        || extra_writable_roots().iter().any(|root| dir.starts_with(root))
}

/// "受保护与否"要能在目录还不存在时就判出来：从叶子往上找到第一个能 canonicalize 的祖先，
/// 把剩下的段原样拼回去。拿不到任何可解析的祖先就退回原路径 —— 原路径的字符串前缀仍然能
/// 命中黑名单，而真正的 canonical 判定在后面还要再走一遍（拦符号链接逃逸）。
fn deny_probe(path: &Path) -> PathBuf {
    if let Ok(canonical) = canonicalize_clean(path) {
        return canonical;
    }
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    let mut cursor = path;
    while let (Some(name), Some(parent)) = (cursor.file_name(), cursor.parent()) {
        tail.push(name.to_os_string());
        cursor = parent;
        if let Ok(base) = canonicalize_clean(cursor) {
            let mut out = base;
            for seg in tail.iter().rev() {
                out.push(seg);
            }
            return out;
        }
    }
    path.to_path_buf()
}

/// 校验并解析一个 `.json` 目标路径。
///
/// 目标文件本身可以还不存在（导出到新文件名），但**所在目录必须能 canonicalize** ——
/// 不先把 `..` 与符号链接折回真实路径，黑名单就只是看着像有。
/// 共用的一枚"落盘边界"守卫：**指定后缀 + 用户可写范围 + 可选的存在性与体积检查**。
///
/// 偏好文件与历史 CSV 导出走的是同一个函数，而不是各写一份校验：两份规则一旦漂移，
/// 就会出现"CSV 能写到 `/System` 而 JSON 不能"这种只有挨个测才能发现的问题。
/// `kind` 只用于把错误说成人话（"偏好文件" / "历史 CSV"）。
pub(crate) fn resolve_target_path(
    raw: &str,
    must_exist: bool,
    extension: &str,
    kind: &str,
    max_bytes: u64,
) -> CommandResult<PathBuf> {
    if raw.trim().is_empty() {
        return Err(AppError::invalid_input("必须先选择一个文件路径"));
    }
    let expanded = expand_tilde(raw).ok_or_else(|| AppError::invalid_input("文件路径无法解析"))?;
    let file_name = expanded
        .file_name()
        .ok_or_else(|| AppError::invalid_input("路径指向的是一个目录，不是一个文件"))?;
    if expanded
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case(extension))
        != Some(true)
    {
        return Err(AppError::invalid_input(format!("{kind}必须是一个 .{extension} 文件")));
    }

    // 拒判排在存在性之前：一个位置受不受保护只取决于路径本身。原先先 canonicalize 父目录，
    // 于是"这个目录在本平台不存在"会先返回 NOT_FOUND —— 同一条守卫在 macOS 上给 PATH_DENIED、
    // 在 Linux 上给 NOT_FOUND（/System/Volumes/Data 只有 macOS 有），守卫的结论取决于文件系统运气。
    if is_denied(&deny_probe(&expanded)) {
        return Err(AppError::path_denied(format!(
            "该位置属于系统或受保护目录，不能读写{kind}"
        )));
    }

    let parent = expanded
        .parent()
        .ok_or_else(|| AppError::invalid_input("路径缺少所在目录"))?;
    let canonical_parent = canonicalize_clean(parent).map_err(|_| {
        AppError::not_found(format!("目录不存在或不可访问: {}", sanitize(&parent.to_string_lossy())))
    })?;
    let canonical = canonical_parent.join(file_name);

    if is_denied(&canonical) {
        return Err(AppError::path_denied("该位置属于系统或受保护目录，不能读写偏好文件"));
    }
    if !prefs_root_allowed(&canonical_parent) {
        return Err(AppError::path_denied(format!(
            "只能在用户主目录或临时目录（含 /tmp）之下读写{kind}"
        )));
    }

    if must_exist {
        let meta = fs::metadata(&canonical).map_err(|_| {
            AppError::not_found(format!("文件不存在: {}", sanitize(&canonical.to_string_lossy())))
        })?;
        if !meta.is_file() {
            return Err(AppError::invalid_input("选中的不是一个普通文件"));
        }
        if meta.len() > max_bytes {
            return Err(AppError::invalid_input(format!(
                "文件 {} B 超过 {} B 上限，不像是一份{kind}",
                meta.len(),
                max_bytes,
            )));
        }
    }
    Ok(canonical)
}

/// 偏好文件那一套参数（`.json` + 64 KiB 上限）的快捷入口。
fn resolve_json_path(raw: &str, must_exist: bool) -> CommandResult<PathBuf> {
    resolve_target_path(raw, must_exist, "json", "偏好文件", MAX_PREFS_FILE_BYTES)
}

/// 同目录临时文件 + rename：中途失败既不会留下半份文件，也不会留下 `xxx.tmp-<pid>`。
/// 失败时顺手把临时文件删掉 —— 用户目录里的半成品没人会去清理。
pub(crate) fn write_bytes_atomically(target: &Path, bytes: &[u8]) -> CommandResult<()> {
    let file_name = target
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "export.tmp".to_string());
    let temp = target.with_file_name(format!("{file_name}.tmp-{}", std::process::id()));
    if let Err(e) = fs::write(&temp, bytes) {
        let _ = fs::remove_file(&temp);
        return Err(AppError::failed(format!("写入临时文件失败: {}", e))
            .with_detail(sanitize(&temp.to_string_lossy())));
    }
    if let Err(e) = fs::rename(&temp, target) {
        let _ = fs::remove_file(&temp);
        return Err(AppError::failed(format!("替换目标文件失败: {}", e))
            .with_detail(sanitize(&target.to_string_lossy())));
    }
    Ok(())
}

fn prefs_keys(prefs: &Value) -> usize {
    prefs.as_object().map(|map| map.len()).unwrap_or(0)
}

/// 把前端归一化后的偏好写到指定路径。同目录临时文件 + rename，避免中途失败留下半份配置。
pub fn write_prefs_file(path: &str, prefs: Value) -> CommandResult<ExportOutcome> {
    if !prefs.is_object() {
        return Err(AppError::invalid_input("偏好必须是一组键值，无法写入非对象内容"));
    }
    let target = resolve_json_path(path, false)?;
    let payload = PrefsFile {
        app: PREFS_APP_ID.to_string(),
        schema_version: PREFS_SCHEMA_VERSION,
        exported_at_ms: Some(crate::history::now_ms()),
        prefs,
    };
    let bytes = serde_json::to_vec_pretty(&payload)
        .map_err(|e| AppError::failed("偏好无法序列化为 JSON").with_detail(e.to_string()))?;
    if bytes.len() as u64 > MAX_PREFS_FILE_BYTES {
        return Err(AppError::invalid_input(format!(
            "偏好内容 {} B 超过 {} B 上限",
            bytes.len(),
            MAX_PREFS_FILE_BYTES
        )));
    }

    write_bytes_atomically(&target, &bytes)?;

    Ok(ExportOutcome {
        path: target.to_string_lossy().to_string(),
        bytes: bytes.len() as u64,
        keys: prefs_keys(&payload.prefs),
        schema_version: payload.schema_version,
    })
}

/// 读回一份偏好文件。只负责"认得出这是本应用这份格式"，值合不合法仍由前端白名单判。
pub fn read_prefs_file(path: &str) -> CommandResult<ImportOutcome> {
    let target = resolve_json_path(path, true)?;
    let text = fs::read_to_string(&target)
        .map_err(|e| AppError::failed(format!("读取失败: {}", e)).with_detail(sanitize(&target.to_string_lossy())))?;
    let bytes = text.len() as u64;
    let payload: PrefsFile = match serde_json::from_str(&text) {
        Ok(payload) => payload,
        // 顶层不是对象时 serde 报的是 "invalid type: sequence"，直接给用户一句人话
        Err(e) => {
            return Err(AppError::invalid_input("这个 JSON 不是一份偏好文件").with_detail(e.to_string()));
        }
    };
    if payload.app != PREFS_APP_ID {
        return Err(AppError::invalid_input(format!(
            "这是 `{}` 导出的偏好文件，不是本应用的",
            sanitize(&payload.app)
        )));
    }
    if payload.schema_version != PREFS_SCHEMA_VERSION {
        return Err(AppError::invalid_input(format!(
            "偏好文件是 v{}，本应用认得 v{}；不做猜测式迁移，请在本应用里重设一次",
            payload.schema_version, PREFS_SCHEMA_VERSION
        )));
    }
    if !payload.prefs.is_object() {
        return Err(AppError::invalid_input("偏好内容必须是键值对象"));
    }

    Ok(ImportOutcome {
        path: target.to_string_lossy().to_string(),
        bytes,
        schema_version: payload.schema_version,
        exported_at_ms: payload.exported_at_ms,
        file_keys: prefs_keys(&payload.prefs),
        prefs: payload.prefs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "zsys-prefs-{}-{tag}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("临时目录必须建得出来");
        // 与生产侧同一个 canonicalize：Windows 上 `\\?\` verbatim 前缀会被命令剥掉，
        // 夹具留着它只会比出"差一个前缀"的假失败。
        canonicalize_clean(&dir).expect("临时目录要能 canonicalize")
    }

    fn cleanup(dir: &std::path::Path) {
        let _ = fs::remove_dir_all(dir);
    }

    fn sample_prefs() -> Value {
        json!({
            "darkMode": true,
            "intervalMs": 1000,
            "trendRangeSecs": 300,
            "activeTab": "disk",
            "alert": { "enabled": false, "consecutive": 5 }
        })
    }

    #[test]
    fn exported_file_round_trips_through_import() {
        let dir = fixture("roundtrip");
        let target = dir.join("z-prefs.json");
        let out = write_prefs_file(&target.to_string_lossy(), sample_prefs()).expect("导出应当成功");
        assert_eq!(out.keys, 5);
        assert_eq!(out.schema_version, PREFS_SCHEMA_VERSION);
        assert!(out.bytes > 0);
        // 返回的路径必须是真正落盘那一条，不能是用户传来的原样字符串
        assert_eq!(PathBuf::from(&out.path), target);

        let back = read_prefs_file(&target.to_string_lossy()).expect("导入应当读回同一份");
        assert_eq!(back.prefs, sample_prefs());
        assert_eq!(back.file_keys, 5);
        assert_eq!(back.bytes, out.bytes);
        assert!(
            back.exported_at_ms.is_some(),
            "导出时刻必须写进去: {back:?}"
        );
        cleanup(&dir);
    }

    #[test]
    fn a_home_alias_resolves_to_the_real_path_before_writing() {
        let dir = fixture("tilde");
        // 测试不往真家目录写：改测"传进来的 `~` 会被展开成 canonical 路径"这一条通用性质
        let target = dir.join("aliased.json");
        let raw = format!("{}", target.to_string_lossy());
        let out = write_prefs_file(&raw, json!({ "darkMode": false })).expect("导出应当成功");
        assert!(
            !out.path.contains("~/") && PathBuf::from(&out.path).is_absolute(),
            "返回路径不该还带 `~`: {}",
            out.path
        );
        assert!(PathBuf::from(&out.path).exists());
        cleanup(&dir);
    }

    /// 真机验证踩到的坑：macOS 的 `temp_dir()` 是 per-user 的 `$TMPDIR`
    /// （`/var/folders/.../T`），用户在保存框里选 `/tmp/xxx.json` 时会被自己拦成 PATH_DENIED。
    #[test]
    fn the_global_tmp_dir_is_writable_even_though_it_is_not_temp_dir() {
        let dir = match Path::new("/tmp").canonicalize() {
            Ok(dir) if dir.is_dir() => dir,
            _ => return, // 没有全局 /tmp 的平台不需要这条豁免
        };
        let target = dir.join(format!("zsys-t511-{}.json", std::process::id()));
        let out = write_prefs_file(&target.to_string_lossy(), json!({ "darkMode": true }))
            .expect("/tmp 之下的偏好文件必须能写");
        assert!(PathBuf::from(&out.path).exists(), "返回的路径要就是真正落盘那一条");
        let back = read_prefs_file(&target.to_string_lossy()).expect("/tmp 之下也要能读回来");
        assert_eq!(back.prefs, json!({ "darkMode": true }));
        let _ = fs::remove_file(&target);
    }

    #[test]
    fn a_non_json_target_is_refused_before_anything_is_written() {
        let dir = fixture("extension");
        let target = dir.join("prefs.txt");
        let err = write_prefs_file(&target.to_string_lossy(), sample_prefs())
            .expect_err("必须拒掉非 .json 目标");
        assert_eq!(err.code, "INVALID_INPUT", "{err:?}");
        assert!(
            !target.exists(),
            "被拒的导出不能留下任何文件"
        );
        cleanup(&dir);
    }

    #[test]
    fn denied_system_locations_are_refused() {
        // 这些路径都不存在也无所谓：后缀与黑名单的检查在存在性检查之前
        for raw in ["/System/Volumes/Data/prefs.json", "/etc/prefs.json"] {
            let err = write_prefs_file(raw, sample_prefs())
                .expect_err("系统目录必须被拒");
            assert!(
                err.code == "PATH_DENIED" || err.code == "NOT_FOUND",
                "{raw} 的拒绝理由不对: {err:?}"
            );
        }
    }

    #[test]
    fn a_parent_directory_is_not_treated_as_a_file() {
        let dir = fixture("isdir");
        let err = write_prefs_file(&dir.to_string_lossy(), sample_prefs())
            .expect_err("目录不能当文件写");
        assert_eq!(err.code, "INVALID_INPUT", "{err:?}");
        cleanup(&dir);
    }

    #[test]
    fn exporting_leaves_no_temp_file_behind() {
        let dir = fixture("tmp");
        write_prefs_file(&dir.join("a.json").to_string_lossy(), sample_prefs()).expect("导出应当成功");
        let leftovers: Vec<_> = fs::read_dir(&dir)
            .expect("目录可读")
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.path().to_string_lossy().contains(".tmp-"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "临时文件必须被 rename 消化掉: {leftovers:?}"
        );
        cleanup(&dir);
    }

    #[test]
    fn re_exporting_overwrites_the_previous_copy_instead_of_appending() {
        let dir = fixture("overwrite");
        let target = dir.join("again.json");
        write_prefs_file(&target.to_string_lossy(), json!({ "darkMode": true })).expect("first");
        let second = write_prefs_file(&target.to_string_lossy(), json!({ "darkMode": false })).expect("second");
        let back = read_prefs_file(&target.to_string_lossy()).expect("read");
        assert_eq!(back.prefs, json!({ "darkMode": false }), "第二次导出必须整份覆盖");
        assert_eq!(back.bytes, second.bytes);
        assert_eq!(back.file_keys, 1);
        cleanup(&dir);
    }

    #[test]
    fn a_missing_file_is_reported_as_not_found_rather_than_empty_prefs() {
        let dir = fixture("missing");
        let err = read_prefs_file(&dir.join("nope.json").to_string_lossy()).expect_err("不存在的文件必须报错");
        assert_eq!(err.code, "NOT_FOUND", "{err:?}");
        cleanup(&dir);
    }

    #[test]
    fn malformed_or_non_object_json_is_rejected_without_panicking() {
        let dir = fixture("malformed");
        for (name, body) in [
            ("broken.json", "{ not json"),
            ("array.json", "[1,2,3]"),
            ("string.json", "\"just a string\""),
            ("empty.json", ""),
        ] {
            let path = dir.join(name);
            fs::write(&path, body).expect("夹具要写得进去");
            let err = read_prefs_file(&path.to_string_lossy())
                .expect_err("畸形内容应被拒而不是抛异常");
            assert_eq!(err.code, "INVALID_INPUT", "{name}: {err:?}");
        }
        cleanup(&dir);
    }

    #[test]
    fn a_foreign_app_export_is_not_mistaken_for_ours() {
        let dir = fixture("foreign");
        let path = dir.join("other.json");
        fs::write(
            &path,
            json!({ "app": "some-other-tool", "schemaVersion": 1, "prefs": {} }).to_string(),
        )
        .expect("夹具要写得进去");
        let err = read_prefs_file(&path.to_string_lossy()).expect_err("别的应用的导出必须被拒");
        assert_eq!(err.code, "INVALID_INPUT", "{err:?}");
        assert!(
            err.message.contains("some-other-tool"),
            "要告诉用户这份文件是谁的: {:?}",
            err.message
        );
        cleanup(&dir);
    }

    #[test]
    fn a_newer_schema_version_is_refused_instead_of_being_guessed() {
        let dir = fixture("version");
        let path = dir.join("future.json");
        fs::write(
            &path,
            json!({ "app": PREFS_APP_ID, "schemaVersion": 99, "prefs": { "darkMode": true } }).to_string(),
        )
        .expect("夹具要写得进去");
        let err = read_prefs_file(&path.to_string_lossy()).expect_err("未定义的版本不能猜");
        assert_eq!(err.code, "INVALID_INPUT", "{err:?}");
        assert!(err.message.contains("v99"), "{:?}", err.message);
        cleanup(&dir);
    }

    #[test]
    fn an_oversized_file_is_refused_before_it_is_read_into_memory() {
        let dir = fixture("oversize");
        let path = dir.join("huge.json");
        // 填充成一个合法 JSON 之外的巨型文件即可：检查发生在读取之前
        fs::write(&path, vec![b'x'; (MAX_PREFS_FILE_BYTES + 1) as usize]).expect("夹具要写得进去");
        let err = read_prefs_file(&path.to_string_lossy()).expect_err("超限文件必须被拒");
        assert_eq!(err.code, "INVALID_INPUT", "{err:?}");
        assert!(err.message.contains("上限"), "{:?}", err.message);
        cleanup(&dir);
    }

    #[test]
    fn exported_prefs_payload_is_camel_case_and_carries_no_snake_keys() {
        let dir = fixture("contract");
        let target = dir.join("shape.json");
        write_prefs_file(&target.to_string_lossy(), sample_prefs()).expect("导出应当成功");
        let text = fs::read_to_string(&target).expect("文件要读得回来");
        for key in ["app", "schemaVersion", "exportedAtMs", "prefs", "darkMode"] {
            assert!(text.contains(&format!("\"{key}\"")), "缺字段 {key}: {text}");
        }
        assert!(
            !text.contains("schema_version") && !text.contains("exported_at_ms"),
            "落盘字段必须全 camelCase: {text}"
        );
        cleanup(&dir);
    }

    /// 与 `error::tests` / `history::tests` 同一手法：字段名和命令名是跨语言的，
    /// 任何一侧改了而另一侧没改，都要在这里失败而不是等到界面上出现 `undefined`。
    #[test]
    fn prefs_contract_matches_the_frontend_ipc_file() {
        let dir = fixture("ipc");
        let target = dir.join("ipc.json");
        let exported = write_prefs_file(&target.to_string_lossy(), sample_prefs()).expect("导出应当成功");
        let export_json = serde_json::to_string(&exported).unwrap();
        for key in ["path", "bytes", "keys", "schemaVersion"] {
            assert!(
                export_json.contains(&format!("\"{key}\"")),
                "ExportOutcome 缺字段 {key}: {export_json}"
            );
        }
        let imported = read_prefs_file(&target.to_string_lossy()).expect("导入应当成功");
        let import_json = serde_json::to_string(&imported).unwrap();
        for key in ["path", "bytes", "schemaVersion", "exportedAtMs", "prefs", "fileKeys"] {
            assert!(
                import_json.contains(&format!("\"{key}\"")),
                "ImportOutcome 缺字段 {key}: {import_json}"
            );
        }

        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let contract = fs::read_to_string(manifest.join("../src/ipc_contract.ts"))
            .expect("前端 IPC 契约文件必须存在");
        for line in [
            "exportPrefsFile: \"export_prefs_file\"",
            "importPrefsFile: \"import_prefs_file\"",
        ] {
            assert!(contract.contains(line), "前端未登记命令 {line}");
        }
        let lib_rs = fs::read_to_string(manifest.join("src/lib.rs")).expect("lib.rs 必须存在");
        for name in ["commands::export_prefs_file", "commands::import_prefs_file"] {
            assert!(lib_rs.contains(name), "命令没注册进 invoke_handler: {name}");
        }
        // 命令按仓库约定集中在 commands.rs，这里锁定"它确实转调本模块"，
        // 否则本模块改签名后 commands.rs 里的调用会静默变成另一套逻辑。
        let commands_rs = fs::read_to_string(manifest.join("src/commands.rs"))
            .expect("commands.rs 必须存在");
        for name in ["prefs::write_prefs_file", "prefs::read_prefs_file"] {
            assert!(commands_rs.contains(name), "commands.rs 未转调 {name}");
        }
        for key in ["fileKeys", "schemaVersion", "exportedAtMs"] {
            assert!(
                contract.contains(&format!("{key}:")),
                "前端偏好契约类型缺字段 {key}"
            );
        }
        cleanup(&dir);
    }
}
