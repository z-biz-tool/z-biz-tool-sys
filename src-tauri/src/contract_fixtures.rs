//! 只给测试用的小工具：从 `src/ipc_contract.ts` 里取 `interface` 的字段名，
//! 与 Rust 侧 `serde` 真正序列化出来的 key 对拍（T5-08 的传感器、T5-02 的通知记账都用它）。
//!
//! 为什么单独立一个文件而不是在两处各写一遍解析：这几条契约测试的价值全在"解析真的读到了字段"，
//! 复制两份就会有两份各自漂移的可能。`the_parser_actually_reads_the_interfaces` 给解析器本身
//! 上锁 —— 一个"永远返回空表"的解析器会让所有对拍测试变成恒真断言。

use serde_json::Value;

/// 去掉 `//` 行注释与 `/* … */` 块注释，保留换行。
/// 必须**先**剥注释再切字段：注释里出现 `;` 时，先按 `;` 切会把注释的后半截留下，
/// 而后半截不再以 `*` 开头，于是"注释里的某个 `name: type`"会被当成字段读进来（本文件的测试就抓到过）。
fn strip_comments(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut chars = source.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '/' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some('*') => {
                chars.next();
                // 块注释：吃到 `*/` 为止，中间的换行补回去，行号才不至于全糊在一行
                while let Some(inner) = chars.next() {
                    if inner == '*' && chars.peek() == Some(&'/') {
                        chars.next();
                        break;
                    }
                    if inner == '\n' {
                        out.push('\n');
                    }
                }
            }
            Some('/') => {
                chars.next();
                for inner in chars.by_ref() {
                    if inner == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            _ => out.push('/'),
        }
    }
    out
}

/// `contract` 里 `interface <name> { … }` 的字段名，按字典序返回（可选符 `?` 已剥掉）。
pub(crate) fn ts_interface_keys(contract: &str, name: &str) -> Vec<String> {
    let clean = strip_comments(contract);
    let head = format!("interface {name} {{");
    let start = clean
        .find(&head)
        .unwrap_or_else(|| panic!("前端契约里没有 {head}"));
    let body = &clean[start + head.len()..];
    let body = &body[..body.find('}').expect("interface 应有结尾的大括号")];
    let mut out: Vec<String> = body
        .split(';')
        .map(|decl| {
            decl
                .split(':')
                .next()
                .unwrap_or_default()
                .trim()
                .trim_end_matches('?')
                .to_string()
        })
        .filter(|field| !field.is_empty())
        .collect();
    out.sort();
    out
}

/// 序列化结果里的 key，同样按字典序。不是对象就直接 panic：那说明 `serialize` 掉了。
pub(crate) fn serialized_keys(value: &Value) -> Vec<String> {
    let mut keys: Vec<String> = value
        .as_object()
        .expect("载荷应是 JSON 对象")
        .keys()
        .cloned()
        .collect();
    keys.sort();
    keys
}

/// 剥掉 ts/tsx 里的 `{/* … */}` 与整行 `// …` 注释，只留用户看得见的文案。
/// 文案审计需要它：像"这里不能说已送达"这种**解释为什么不许说**的注释，
/// 会把审计自己的关键词命中一遍（T5-08 那轮已经为此改过一次判级审计）。
pub(crate) fn strip_ts_comments(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut rest = source;
    while let Some(start) = rest.find("{/*") {
        let tail = &rest[start + 3..];
        match tail.find("*/}") {
            Some(end) => {
                out.push_str(&rest[..start]);
                rest = &tail[end + 3..];
            }
            // 没闭合就当没有这段注释，宁可留下文本也不静默吞掉后面的内容
            None => {
                out.push_str(rest);
                return out;
            }
        }
    }
    out.push_str(rest);
    out.lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
export interface Alpha {
  one: string;
  /** 注释里也写着 two: number; 不该被当成字段 */
  two: number | null;
  // 行注释里再来一个 fake: string;} 也不该被读到，收尾大括号在注释里同样不算数
  three?: boolean;
}

export interface Beta {
  items: Alpha[];
}
"#;

    /// 解析器本身要测：它一旦"返回空表"或"把注释里的字面量当字段"，两条对拍契约就都成了摆设。
    #[test]
    fn the_parser_actually_reads_the_interfaces() {
        assert_eq!(ts_interface_keys(SAMPLE, "Alpha"), ["one", "three", "two"]);
        assert_eq!(ts_interface_keys(SAMPLE, "Beta"), ["items"]);
        // 注释里那句 `two: number;` 只能出现一次，说明注释整段被剥掉了。
        assert_eq!(ts_interface_keys(SAMPLE, "Alpha").iter().filter(|k| *k == "two").count(), 1);
        // 两种注释形式都要剥干净：块注释里的 `two` 与行注释里的 `fake`。
        assert!(!ts_interface_keys(SAMPLE, "Alpha").iter().any(|k| k.contains("fake")));
        // 可选符不能混进字段名。
        assert!(!ts_interface_keys(SAMPLE, "Alpha").iter().any(|k| k.ends_with('?')));
    }

    /// 对**真实**契约文件也得读出字段：`SAMPLE` 之外若只测样例，哪天 `ipc_contract.ts` 的写法
    /// 变得让解析器读空了，所有对拍测试会一起变成"空 == 空"的摆设。
    #[test]
    fn it_reads_the_real_contract_file_too() {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let contract = std::fs::read_to_string(manifest.join("../src/ipc_contract.ts")).unwrap();
        assert_eq!(
            ts_interface_keys(&contract, "NotifyStatus"),
            ["deliveryIsReported", "failed", "lastError", "submitted", "suppressed"]
        );
        assert_eq!(
            ts_interface_keys(&contract, "ThermalReport"),
            ["reason", "sensors"]
        );
        assert_eq!(
            ts_interface_keys(&contract, "ThermalSensor"),
            ["critical", "kind", "label", "severity", "source", "value"]
        );
    }

    /// key 表必须是**排序过**的：两条契约测试都直接比 `Vec<String>`，顺序不固定就会偶发假红。
    #[test]
    fn both_key_lists_come_back_sorted() {
        assert_eq!(
            serialized_keys(&serde_json::json!({"delta": 1, "alpha": 2, "Charlie": 3})),
            ["Charlie", "alpha", "delta"],
            "字节序排序：大写在前，与小写混排时的顺序要固定"
        );
        assert_eq!(serialized_keys(&serde_json::json!({})).len(), 0, "空对象就是没有 key");
    }

    /// 剥注释只剥注释：注释里的关键词不该被文案审计读到，但**同一段里注释之后的正文必须留着**。
    #[test]
    fn comment_stripping_keeps_the_visible_copy() {
        let source = "  {/* 这里不能说已送达 */}\n  <Text>已提交 N 条</Text>\n  // 说明：不是已送达\n  <b>兜底</b>\n";
        let visible = strip_ts_comments(source);
        assert!(!visible.contains("已送达"), "注释里的说明被当成正文了：{visible}");
        assert!(visible.contains("已提交 N 条"), "正文被一起吃掉了：{visible}");
        assert!(visible.contains("<b>兜底</b>"), "注释之后的行丢了：{visible}");
        // 没闭合的注释不许把后面全部内容吞掉（那会让文案审计静默变成空扫）
        let broken = "/* 忘了闭合\n  <Text>重要文案</Text>";
        assert!(strip_ts_comments(broken).contains("重要文案"), "未闭合注释导致后文丢失");
    }
}
