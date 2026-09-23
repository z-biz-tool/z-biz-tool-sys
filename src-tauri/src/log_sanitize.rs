use regex::Regex;
use std::sync::OnceLock;

fn home_dir_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(/Users/|/home/|\\Users\\)[^/\\]+").unwrap())
}

fn ipv4_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\b(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})\b").unwrap()
    })
}

fn secret_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?i)\b(password|passwd|pwd|token|secret|api_?key|access_key|private_key)\b(\s*[=:]\s*)("[^"]*"|'[^']*'|[^\s;,]+)"#)
.unwrap()
    })
}

/// 返回给前端 / 写入日志的文本先脱敏：错误信息常带完整用户路径、局域网 IP 与凭据。
pub fn sanitize(input: &str) -> String {
    let step = secret_re().replace_all(input, |c: &regex::Captures| {
        format!("{}{}***", c.get(1).unwrap().as_str(), c.get(2).unwrap().as_str())
    });
    let step = home_dir_re().replace_all(&step, "$1<user>");
    ipv4_re()
        .replace_all(&step, |c: &regex::Captures| {
            let octets: Vec<u32> = (1..=4)
                .filter_map(|i| c.get(i).and_then(|m| m.as_str().parse::<u32>().ok()))
                .collect();
            // 只有形似地址（每段 ≤255）才脱敏，避免把版本号 1.2.3.4 之外的数字误伤。
            if octets.len() == 4 && octets.iter().all(|o| *o <= 255) {
                format!("{}.{}.*.*", octets[0], octets[1])
            } else {
                c.get(0).unwrap().as_str().to_string()
            }
        })
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_home_directory_username() {
        assert_eq!(
            sanitize("/Users/zifang/.cache/x: permission denied"),
            "/Users/<user>/.cache/x: permission denied"
        );
        assert_eq!(
            sanitize("open failed: /home/rootsec/tmp"),
            "open failed: /home/<user>/tmp"
        );
    }

    #[test]
    fn masks_host_part_of_ipv4() {
        assert_eq!(sanitize("addr 192.168.1.234 down"), "addr 192.168.*.* down");
        assert_eq!(sanitize("10.0.3.9"), "10.0.*.*");
    }

    #[test]
    fn leaves_short_numbers_alone() {
        assert_eq!(sanitize("size 1024 bytes"), "size 1024 bytes");
        assert_eq!(sanitize("elapsed 12.3 ms"), "elapsed 12.3 ms");
    }

    #[test]
    fn masks_secret_values() {
        assert_eq!(
            sanitize("connect with password=hunter2 to db"),
            "connect with password=*** to db"
        );
        assert_eq!(sanitize("token: \"abcdef123456\""), "token: ***");
        assert_eq!(sanitize("PASSWORD=xyz"), "PASSWORD=***");
        assert_eq!(
            sanitize("export API_KEY=supersecret;"),
            "export API_KEY=***;"
        );
    }

    #[test]
    fn keeps_text_without_secrets() {
        assert_eq!(sanitize("no sensitive data here"), "no sensitive data here");
    }

    /// 取 `(` 之后第一段配对括号的文本（用于看一条日志到底把什么值打出去）。
    fn call_args(source: &str) -> String {
        let mut depth = 1usize;
        let mut out = String::new();
        for c in source.chars() {
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            out.push(c);
        }
        out
    }

    /// 日志的第一个字符串字面量（≈这条日志的前缀标识）。
    fn first_literal(args: &str) -> String {
        let Some(start) = args.find('"') else {
            return String::new();
        };
        let rest = &args[start + 1..];
        match rest.find('"') {
            Some(end) => rest[..end].to_string(),
            None => rest.to_string(),
        }
    }

    /// SEC-V08：生产代码里**每一条**日志都必须满足"值过 `sanitize`"或"在白名单里并写明为什么安全"。
    ///
    /// 这一条把 04 表里长期写着"⚠ 部分覆盖"的检查项变成了门禁：2026-09-23 审计时找到五处在
    /// 直接把原始值打出去的日志（磁盘告警的挂载点、用户输入的搜索关键字、两处 emit 的 io 错误、
    /// 两处历史读失败的 io 错误），全部补了脱敏；剩下的确实不含路径/机密的，只能进这个白名单，
    /// 且必须写着理由 —— 以后加日志的人要么 sanitize，要么来改这张表并说清原因。
    #[test]
    fn sec_v08_production_log_lines_are_sanitized_or_allowlisted() {
        fn collect(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).expect("src 目录应存在") {
                let path = entry.expect("可读目录项").path();
                if path.is_dir() {
                    collect(&path, out);
                } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                    out.push(path);
                }
            }
        }
        const ALLOWED: &[(&str, &str)] = &[
            ("[monitor] {} 首帧: cpu=", "只有百分比、核心数与分区/网卡条数（{} 是事件名）"),
            ("[monitor] 采集帧 panic", "原因取自 panic_reason()，那里已经 sanitize 过"),
            ("[monitor] 进程帧 panic", "同上"),
            ("[notify] ", "Err 分支的原文在 map_err 里就 sanitize 过"),
        ];

        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut files = Vec::new();
        collect(&manifest.join("src"), &mut files);
        assert!(files.len() > 15, "只找到 {} 个源文件，递归没生效", files.len());

        let mut checked = 0usize;
        let mut offenders = Vec::new();
        for file in &files {
            let text = std::fs::read_to_string(file).expect("源码可读");
            // 只看生产段：测试里的 println! 打的是夹具数据，且测试文件本身不该被日志门禁管
            let production = text.split("#[cfg(test)]").next().unwrap_or("");
            // 只找 `println!(`，再把前面是 `e` 的（即 `eprintln!`）也认下来：
            // 分开找两个 marker 会让每条 eprintln! 被数两遍（`eprintln!(` 里含着 `println!(`）。
            {
                let mut from = 0usize;
                while let Some(rel) = production[from..].find("println!(") {
                    let start = from + rel + "println!(".len();
                    let args = call_args(&production[start..]);
                    let literal = first_literal(&args);
                    checked += 1;
                    let sanitized = args.contains("sanitize(");
                    let listed = ALLOWED
                        .iter()
                        .any(|(prefix, _)| literal.starts_with(*prefix));
                    if !(sanitized || listed) {
                        offenders.push(format!(
                            "{}: {}",
                            file.file_name().unwrap().to_string_lossy(),
                            literal.chars().take(60).collect::<String>()
                        ));
                    }
                    from = start;
                }
            }
        }
        assert!(checked >= 8, "只扫到 {checked} 条日志，扫描逻辑可能失效");
        assert!(offenders.is_empty(), "这些日志点既没脱敏也不在白名单里：{offenders:?}");
    }
}
