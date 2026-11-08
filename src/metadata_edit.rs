//! Small source editor for the native aggregate common.api option.
use anyhow::{ensure, Context, Result};
use std::{
    collections::BTreeSet,
    hash::{Hash, Hasher},
};
#[derive(Debug)]
struct Token {
    start: usize,
    end: usize,
    text: String,
    quoted: bool,
}
fn tokens(text: &str) -> Result<Vec<Token>> {
    let b = text.as_bytes();
    let mut i = 0;
    let mut out = vec![];
    while i < b.len() {
        if b[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if b[i..].starts_with(b"//") {
            while i < b.len() && b[i] != b'\n' {
                i += 1
            }
            continue;
        }
        if b[i..].starts_with(b"/*") {
            i += 2;
            while i + 1 < b.len() && !b[i..].starts_with(b"*/") {
                i += 1
            }
            ensure!(i + 1 < b.len(), "unterminated comment");
            i += 2;
            continue;
        }
        let start = i;
        let quoted = matches!(b[i], b'\'' | b'"');
        if quoted {
            let quote = b[i];
            i += 1;
            while i < b.len() && b[i] != quote {
                if b[i] == b'\\' {
                    i += 1
                }
                i += 1
            }
            ensure!(i < b.len(), "unterminated string");
            i += 1
        } else if b[i].is_ascii_alphabetic() || b[i] == b'_' {
            i += 1;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1
            }
        } else {
            i += 1;
            while !text.is_char_boundary(i) {
                i += 1
            }
        }
        out.push(Token {
            start,
            end: i,
            text: text[start..i].into(),
            quoted,
        });
    }
    Ok(out)
}
pub fn revision(text: &str) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    format!("{:016x}", h.finish())
}
pub fn update(text: &str, title: &str, tags: &[String]) -> Result<String> {
    ensure!(
        !title.trim().is_empty() && title.chars().count() <= 240,
        "标题不能为空，最多 240 字符"
    );
    ensure!(tags.len() <= 64, "最多 64 个标签");
    let mut seen = BTreeSet::new();
    for tag in tags {
        ensure!(
            !tag.trim().is_empty()
                && tag == tag.trim()
                && tag.chars().count() <= 120
                && !tag.contains(['\r', '\n', '\0'])
                && seen.insert(tag),
            "标签为空、重复、过长或含非法字符"
        );
    }
    let ts = tokens(text)?;
    let prefix = ["option", "(", "common", ".", "api", ")", "=", "{"];
    let pos = ts
        .windows(prefix.len())
        .position(|w| w.iter().zip(prefix).all(|(t, p)| t.text == p))
        .context("需要原生 option (common.api) = {...} 格式")?;
    let open = pos + prefix.len() - 1;
    let mut depth = 1;
    let mut i = open + 1;
    let mut edits = vec![];
    while i < ts.len() && depth > 0 {
        let token = &ts[i];
        if depth == 1 && !token.quoted && matches!(token.text.as_str(), "title" | "tags") {
            ensure!(
                ts.get(i + 1).is_some_and(|t| t.text == ":"),
                "标题/标签选项需要冒号"
            );
            let mut j = i + 2;
            let value = ts.get(j).context("missing option value")?;
            if value.quoted {
                while ts.get(j).is_some_and(|t| t.quoted) {
                    j += 1
                }
            } else if value.text == "[" {
                j += 1;
                while ts.get(j).is_some_and(|t| t.text != "]") {
                    ensure!(ts[j].quoted || ts[j].text == ",", "invalid tag list");
                    j += 1
                }
                ensure!(j < ts.len(), "unterminated tag list");
                j += 1
            } else {
                anyhow::bail!("标题与标签必须是字符串")
            }
            if ts
                .get(j)
                .is_some_and(|t| matches!(t.text.as_str(), "," | ";"))
            {
                j += 1
            }
            let mut start = token.start;
            let mut end = ts[j - 1].end;
            let line_start = text[..start].rfind('\n').map(|i| i + 1).unwrap_or(0);
            let line_end = text[end..]
                .find('\n')
                .map(|i| end + i)
                .unwrap_or(text.len());
            if text[line_start..start].trim().is_empty() && text[end..line_end].trim().is_empty() {
                start = line_start;
                end = if line_end < text.len() {
                    line_end + 1
                } else {
                    line_end
                };
            }
            edits.push((start, end));
            i = j;
            continue;
        }
        if !token.quoted {
            if token.text == "{" {
                depth += 1
            } else if token.text == "}" {
                depth -= 1
            }
        }
        i += 1;
    }
    ensure!(depth == 0, "unterminated common.api option");
    let mut result = text.to_owned();
    for (a, b) in edits.into_iter().rev() {
        result.replace_range(a..b, "")
    }
    let mut insertion = format!("\n  title: {}", serde_json::to_string(title.trim())?);
    for tag in tags {
        insertion.push_str(&format!("\n  tags: {}", serde_json::to_string(tag)?));
    }
    insertion.push_str("\n  ");
    let from = ts[open].end;
    let whitespace = result[from..]
        .bytes()
        .take_while(u8::is_ascii_whitespace)
        .count();
    result.replace_range(from..from + whitespace, &insertion);
    Ok(result
        .lines()
        .map(|line| if line.trim().is_empty() { "" } else { line })
        .collect::<Vec<_>>()
        .join("\n")
        + if result.ends_with('\n') { "\n" } else { "" })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn changes_only_selected_options_with_quotes_nested_fields_and_comments() {
        let input = r#"syntax="proto3";
// title: "comment"
option (common.api) = { title: "old" tags:["a","b"]
 auth: SESSION description: "quotes: \"title: x\" { }" // keep comment
 docs:{status:"testing"} examples:{name:"a" request_json:'{"title":"keep","tags":[]}'}
 maintainers:"jim"
};
message in { string title=1; } message out {}"#;
        let out = update(input, "标题 \"引号\"", &["mp_api".into(), "中文".into()]).unwrap();
        assert!(out.contains("title: \"标题 \\\"引号\\\"\""));
        assert!(out.contains("tags: \"mp_api\""));
        assert!(out.contains("request_json:'{\"title\":\"keep\",\"tags\":[]}'"));
        assert!(out.contains("// keep comment"));
        assert!(out.contains("string title=1"));
        assert!(out.contains("auth: SESSION"));
        assert_ne!(revision(input), revision(&out));
        assert_eq!(
            update(&out, "标题 \"引号\"", &["mp_api".into(), "中文".into()]).unwrap(),
            out
        );
    }
    #[test]
    fn rejects_invalid_edits() {
        assert!(update("", "x", &[]).is_err());
        assert!(update("", "", &[]).is_err());
        assert!(update("", "x", &["a".into(), "a".into()]).is_err());
    }
}
