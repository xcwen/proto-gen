use anyhow::{ensure, Result};
use prost_types::FileDescriptorProto;
fn offsets(text: &str) -> Vec<usize> {
    std::iter::once(0)
        .chain(text.match_indices('\n').map(|(i, _)| i + 1))
        .collect()
}
fn range(span: &[i32], lines: &[usize]) -> (usize, usize) {
    (
        lines[span[0] as usize] + span[1] as usize,
        if span.len() == 3 {
            lines[span[0] as usize] + span[2] as usize
        } else {
            lines[span[2] as usize] + span[3] as usize
        },
    )
}
fn items(text: &str, start: usize, end: usize) -> Vec<(usize, usize)> {
    let b = text.as_bytes();
    let mut out = vec![];
    let mut from = start + 1;
    let mut depth = 0;
    let mut quote = 0;
    let mut escape = false;
    let mut line = false;
    let mut block = false;
    let mut i = start + 1;
    while i < end - 1 {
        let c = b[i];
        if line {
            if c == b'\n' {
                line = false;
            }
        } else if block {
            if c == b'*' && b.get(i + 1) == Some(&b'/') {
                block = false;
                i += 1;
            }
        } else if quote != 0 {
            if escape {
                escape = false;
            } else if c == b'\\' {
                escape = true;
            } else if c == quote {
                quote = 0;
            }
        } else if c == b'/' && b.get(i + 1) == Some(&b'/') {
            line = true;
            i += 1;
        } else if c == b'/' && b.get(i + 1) == Some(&b'*') {
            block = true;
            i += 1;
        } else if c == b'"' || c == b'\'' {
            quote = c;
        } else if b"[{(<".contains(&c) {
            depth += 1;
        } else if b"]})>".contains(&c) {
            depth -= 1;
        } else if c == b',' && depth == 0 {
            out.push((from, i));
            from = i + 1;
        }
        i += 1;
    }
    out.push((from, end - 1));
    out
}
pub fn strip(text: &str, file: &FileDescriptorProto) -> Result<String> {
    let lines = offsets(text);
    let locs = &file.source_code_info.as_ref().unwrap().location;
    let mut edits = std::collections::BTreeMap::new();
    for l in locs {
        let p = &l.path;
        let remove = (p.len() == 2 && p[0] == 8 && [50001, 50003].contains(&p[1]))
            || (p.len() == 2
                && p[0] == 3
                && [
                    "common/api_options.proto",
                    "common/field_options.proto",
                    "common/doc_fields.proto",
                ]
                .contains(&file.dependency[p[1] as usize].as_str()));
        if remove {
            let (a, b) = range(&l.span, &lines);
            edits.insert(a, (b, String::new()));
        }
    }
    let mut wrappers = std::collections::BTreeMap::<Vec<i32>, Vec<(usize, usize)>>::new();
    for l in locs {
        if let Some(i) = l.path.iter().position(|n| *n == 50002) {
            ensure!(
                l.path.len() == i + 1,
                "use aggregate (common.field) = {{...}} options"
            );
            wrappers
                .entry(l.path[..i].to_vec())
                .or_default()
                .push(range(&l.span, &lines));
        }
    }
    for (path, removed) in wrappers {
        let loc = locs.iter().find(|l| l.path == path).unwrap();
        let (a, b) = range(&loc.span, &lines);
        let keep: Vec<_> = items(text, a, b)
            .into_iter()
            .filter(|(s, e)| !removed.iter().any(|(x, y)| *s <= *x && *y <= *e))
            .map(|(s, e)| text[s..e].trim())
            .filter(|s| !s.is_empty())
            .collect();
        edits.insert(
            a,
            (
                b,
                if keep.is_empty() {
                    String::new()
                } else {
                    format!("[{}]", keep.join(", "))
                },
            ),
        );
    }
    let mut result = text.to_owned();
    for (a, (b, replacement)) in edits.into_iter().rev() {
        result.replace_range(a..b, &replacement);
    }
    Ok(result)
}
