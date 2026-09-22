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
}
