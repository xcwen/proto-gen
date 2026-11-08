use anyhow::{Context, Result};
use prost_types::{DescriptorProto, EnumDescriptorProto, FileDescriptorProto};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

fn comment(file: &FileDescriptorProto, path: &[i32], php: bool) -> String {
    let s = file
        .source_code_info
        .as_ref()
        .and_then(|info| info.location.iter().find(|l| l.path == path))
        .map(|l| {
            l.trailing_comments
                .as_ref()
                .filter(|s| !s.is_empty())
                .or(l.leading_comments.as_ref())
                .cloned()
                .unwrap_or_default()
        })
        .unwrap_or_default();
    if php {
        let leading = file
            .source_code_info
            .as_ref()
            .and_then(|i| i.location.iter().find(|l| l.path == path))
            .and_then(|l| l.leading_comments.as_ref())
            .filter(|s| !s.is_empty());
        leading.unwrap_or(&s).trim().replace('@', "\\@")
    } else {
        s.trim().to_owned()
    }
}
fn scalar(field: &prost_types::FieldDescriptorProto) -> String {
    if field.r#type == Some(11) || field.r#type == Some(14) {
        return field
            .type_name
            .as_deref()
            .unwrap_or_default()
            .trim_start_matches('.')
            .trim_start_matches("proto.project.")
            .to_owned();
    }
    [
        "", "double", "float", "int64", "uint64", "int32", "fixed64", "fixed32", "bool", "string",
        "group", "message", "bytes", "uint32", "enum", "sfixed32", "sfixed64", "sint32", "sint64",
    ]
    .get(field.r#type.unwrap_or(0) as usize)
    .unwrap_or(&"")
    .to_string()
}
fn node_type(t: &str) -> String {
    match t {
        "uint32" | "int32" | "uint64" | "int64" => "integer".into(),
        "float" | "double" => "number".into(),
        "bool" => "boolean".into(),
        _ => t.into(),
    }
}
fn key(file: &FileDescriptorProto, msg: &str, php: bool) -> String {
    if php {
        if file.name().starts_with("common/") {
            format!("{}.{}", file.package(), msg)
        } else {
            format!(
                "{}.{}",
                file.name()
                    .trim_end_matches(".proto")
                    .trim_start_matches("project/")
                    .replace('/', "."),
                msg
            )
        }
    } else {
        format!(
            "{}.{}",
            file.name().trim_end_matches(".proto").replace('/', "."),
            msg
        )
    }
}
fn message(
    map: &mut BTreeMap<String, Value>,
    file: &FileDescriptorProto,
    msg: &DescriptorProto,
    name: &str,
    path: &[i32],
    php: bool,
    models: &crate::field_model::Fields,
) -> Result<()> {
    let mut rows = vec![];
    for (i, f) in msg.field.iter().enumerate() {
        let oneof = f.oneof_index.filter(|_| f.proto3_optional != Some(true));
        if php && oneof.is_some() && name != "apidoc_in" {
            continue;
        }
        let mut p = path.to_vec();
        p.extend([2, i as i32]);
        let full = format!("{}.{name}.{}", file.package(), f.name());
        let model = models
            .get(&full)
            .with_context(|| format!("missing resolved metadata {full}"))?;
        let desc = if php {
            model.description.replace('@', "\\@")
        } else {
            model.description.clone()
        };
        let mut cfg = Map::new();
        if let Some(v) = &model.default {
            cfg.insert("default".into(), v.clone());
        }
        if let Some(v) = model.examples.first() {
            cfg.insert("example".into(), v.clone());
        }
        if let Some(v) = &model.enum_values {
            cfg.insert("enum".into(), json!(v));
        }
        if model.options["upload"] == true {
            cfg.insert("field_type".into(), json!("file"));
        }
        let mut rules = Map::new();
        if !cfg.is_empty() {
            rules.insert(
                "cfg".into(),
                if php {
                    crate::compiler::php_arrays(json!(cfg))
                } else {
                    json!(cfg)
                },
            );
        }

        if let Some(index) = oneof {
            rules.insert("oneof".into(), json!(msg.oneof_decl[index as usize].name()));
        }
        let mut repeated = if f.label == Some(3) { "repeated" } else { "" };
        let mut typ = scalar(f);
        let mut mapkey = Value::Null;
        if f.r#type == Some(11) {
            if let Some(nested) = msg.nested_type.iter().find(|n| {
                f.type_name().ends_with(&format!(".{}", n.name()))
                    && n.options
                        .as_ref()
                        .is_some_and(|o| o.map_entry == Some(true))
            }) {
                repeated = "";
                mapkey = json!(if php {
                    scalar(&nested.field[0])
                } else {
                    node_type(&scalar(&nested.field[0]))
                });
                typ = scalar(&nested.field[1]);
            }
        }
        if php && f.proto3_optional == Some(true) {
            typ = format!("optional {typ}");
        }
        if !php {
            typ = rules
                .get("cfg")
                .and_then(|c| c.get("field_type"))
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| node_type(&typ));
        }
        let rules = if php && rules.is_empty() {
            json!([])
        } else {
            json!(rules)
        };
        rows.push(json!([
            repeated,
            typ,
            f.name(),
            f.number().to_string(),
            desc,
            mapkey,
            rules
        ]));
    }
    map.insert(key(file, name, php), json!(rows));
    for (i, n) in msg.nested_type.iter().enumerate() {
        if n.options
            .as_ref()
            .is_some_and(|o| o.map_entry == Some(true))
        {
            continue;
        }
        let mut p = path.to_vec();
        p.extend([3, i as i32]);
        message(
            map,
            file,
            n,
            &format!("{name}.{}", n.name()),
            &p,
            php,
            models,
        )?;
    }
    Ok(())
}
fn enumeration(
    map: &mut BTreeMap<String, Value>,
    file: &FileDescriptorProto,
    e: &EnumDescriptorProto,
    path: &[i32],
    php: bool,
) {
    if !php {
        return;
    }
    let rows: Vec<_> = e
        .value
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let mut p = path.to_vec();
            p.extend([2, i as i32]);
            json!([
                "",
                "",
                v.name(),
                v.number().to_string(),
                comment(file, &p, php),
                null,
                []
            ])
        })
        .collect();
    map.insert(key(file, e.name(), php), json!(rows));
}
pub fn struct_map(
    workspace: &crate::compiler::Workspace,
    php: bool,
) -> Result<BTreeMap<String, Value>> {
    let mut map = BTreeMap::new();
    for file in workspace.pool.files() {
        let f = file.file_descriptor_proto();
        for (i, m) in f.message_type.iter().enumerate() {
            message(
                &mut map,
                f,
                m,
                m.name(),
                &[4, i as i32],
                php,
                &workspace.fields,
            )?;
        }
        for (i, e) in f.enum_type.iter().enumerate() {
            enumeration(&mut map, f, e, &[5, i as i32], php);
        }
    }
    Ok(map)
}
pub fn errors(workspace: &crate::compiler::Workspace) -> Value {
    let mut map = Map::new();
    for file in workspace.pool.files() {
        let f = file.file_descriptor_proto();
        for (i, e) in f.enum_type.iter().enumerate() {
            if e.name() != "enum_error" {
                continue;
            }
            for (j, v) in e.value.iter().enumerate() {
                map.insert(v.number().to_string(),json!({"value":v.number().to_string(),"name":v.name(),"desc":comment(f,&[5,i as i32,2,j as i32],true)}));
            }
        }
    }
    json!(map)
}
