use anyhow::{bail, ensure, Context, Result};
use prost_reflect::{DescriptorPool, FieldDescriptor, Kind};
use serde_json::{json, Map, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

pub const SCHEMA: &str = include_str!("../schema/common/field_options.proto");
#[derive(Clone, Debug, Default)]
pub struct FieldModel {
    pub description: String,
    pub default: Option<Value>,
    pub examples: Vec<Value>,
    pub nullable: bool,
    pub enum_values: Option<Vec<Value>>,
    pub enum_number: bool,
    pub options: Value,
    pub optional: bool,
    pub items: Option<Box<FieldModel>>,
}
pub type Fields = BTreeMap<String, FieldModel>;
fn options(value: prost_reflect::Value) -> Result<Value> {
    match value {
        prost_reflect::Value::Message(m) => Ok(serde_json::to_value(m)?),
        _ => bail!("metadata must be a message"),
    }
}
fn comments(field: &FieldDescriptor) -> String {
    let file = field.parent_file();
    file.file_descriptor_proto()
        .source_code_info
        .as_ref()
        .and_then(|s| s.location.iter().find(|l| l.path == field.path()))
        .map(|l| {
            l.trailing_comments
                .as_ref()
                .filter(|s| !s.is_empty())
                .or(l.leading_comments.as_ref())
                .cloned()
                .unwrap_or_default()
        })
        .unwrap_or_default()
        .trim()
        .into()
}
fn merge(mut base: Value, local: &Value) -> Value {
    if !base.is_object() {
        base = json!({});
    }
    let b = base.as_object_mut().unwrap();
    if local["clearDefault"] == true {
        b.remove("default");
        b.remove("defaultNull");
    }
    for what in local["clear"].as_array().into_iter().flatten() {
        let keys: &[&str] = match what.as_str().unwrap() {
            "CLEAR_DEFAULT" => &["default", "defaultNull"],
            "CLEAR_EXAMPLES" => &["examples", "examplesNull"],
            "CLEAR_ENUM" => &["enumNumber", "enumStr", "enumJson"],
            "CLEAR_FORMAT" => &["format"],
            "CLEAR_PATTERN" => &["pattern"],
            _ => unreachable!(),
        };
        for key in keys {
            b.remove(*key);
        }
    }
    for (k, v) in local.as_object().unwrap() {
        if k == "default" {
            b.remove("defaultNull");
        }
        if k == "defaultNull" && *v == true {
            b.remove("default");
        }
        if k == "examples" {
            b.remove("examplesNull");
        }
        if k == "examplesNull" && *v == true && local.get("examples").is_none() {
            b.remove("examples");
        }
        if k == "enumNumber" {
            b.remove("enumStr");
            b.remove("enumJson");
        }
        if k == "enumStr" {
            b.remove("enumNumber");
            b.remove("enumJson");
        }
        if k == "enumJson" {
            b.remove("enumNumber");
            b.remove("enumStr");
        }
        if k == "items" {
            b.insert(k.clone(), merge(b.get(k).cloned().unwrap_or(json!({})), v));
        } else {
            b.insert(k.clone(), v.clone());
        }
    }
    b.remove("clearDefault");
    b.remove("clear");
    base
}
fn resolve(
    raw: &Value,
    templates: &BTreeMap<String, Value>,
    stack: &mut BTreeSet<String>,
) -> Result<Value> {
    ensure!(
        !(raw["defaultNull"] == true && raw.get("default").is_some()),
        "default and default_null are mutually exclusive"
    );
    let mut base = json!({});
    if let Some(name) = raw.get("templateRef").and_then(Value::as_str) {
        ensure!(stack.insert(name.into()), "cyclic field template: {name}");
        base = resolve(
            templates
                .get(name)
                .with_context(|| format!("unknown template {name}"))?,
            templates,
            stack,
        )?;
        stack.remove(name);
    }
    base = merge(base, raw);
    if let Some(name) = raw.get("descriptionRef").and_then(Value::as_str) {
        ensure!(
            !raw.as_object().unwrap().contains_key("description"),
            "description and description_ref are mutually exclusive"
        );
        ensure!(
            stack.insert(name.into()),
            "cyclic description template: {name}"
        );
        let resolved = resolve(
            templates
                .get(name)
                .with_context(|| format!("unknown description template {name}"))?,
            templates,
            stack,
        )?;
        stack.remove(name);
        base["description"] = resolved.get("description").cloned().unwrap_or(json!(""));
    }
    if let Some(items) = base.get("items").cloned() {
        base["items"] = resolve(&items, templates, stack)?;
    }
    Ok(base)
}
fn leaf(field: &FieldDescriptor) -> Kind {
    if field.is_map() {
        if let Kind::Message(m) = field.kind() {
            return m.get_field_by_name("value").unwrap().kind();
        }
    }
    field.kind()
}
fn integer(kind: &Kind) -> bool {
    matches!(
        kind,
        Kind::Int32
            | Kind::Sint32
            | Kind::Sfixed32
            | Kind::Uint32
            | Kind::Fixed32
            | Kind::Int64
            | Kind::Sint64
            | Kind::Sfixed64
            | Kind::Uint64
            | Kind::Fixed64
    )
}
pub fn int64(kind: &Kind) -> bool {
    matches!(
        kind,
        Kind::Int64 | Kind::Sint64 | Kind::Sfixed64 | Kind::Uint64 | Kind::Fixed64
    )
}
fn json_number(v: &Value) -> Option<String> {
    if let Some(i) = v.as_i64() {
        Some(i.to_string())
    } else if let Some(i) = v.as_u64() {
        Some(i.to_string())
    } else if let Some(s) = v.as_str() {
        let n: serde_json::Number = serde_json::from_str(s).ok()?;
        if n.is_i64() || n.is_u64() {
            Some(n.to_string())
        } else {
            None
        }
    } else {
        None
    }
}
fn integral(v: &Value) -> Option<i128> {
    if let Some(n) = v.as_i64() {
        Some(n as i128)
    } else if let Some(n) = v.as_u64() {
        Some(n as i128)
    } else {
        v.as_str()?.parse().ok()
    }
}
fn integer_range(kind: &Kind) -> Option<(i128, i128)> {
    Some(match kind {
        Kind::Int32 | Kind::Sint32 | Kind::Sfixed32 => (i32::MIN as i128, i32::MAX as i128),
        Kind::Uint32 | Kind::Fixed32 => (0, u32::MAX as i128),
        Kind::Int64 | Kind::Sint64 | Kind::Sfixed64 => (i64::MIN as i128, i64::MAX as i128),
        Kind::Uint64 | Kind::Fixed64 => (0, u64::MAX as i128),
        _ => return None,
    })
}
fn scalar_matches(v: &Value, kind: &Kind) -> bool {
    if let Some((min, max)) = integer_range(kind) {
        return (if int64(kind) {
            v.as_str().is_some_and(|s| {
                regex::Regex::new(r"^-?(0|[1-9][0-9]*)$")
                    .unwrap()
                    .is_match(s)
            })
        } else {
            v.is_i64() || v.is_u64()
        }) && integral(v).is_some_and(|n| (min..=max).contains(&n));
    }
    match kind {
        Kind::String | Kind::Bytes => v.is_string(),
        Kind::Bool => v.is_boolean(),
        Kind::Float | Kind::Double => v.is_number(),
        Kind::Message(m) => match m.full_name() {
            "google.protobuf.Timestamp" | "google.protobuf.Duration" => v.is_string(),
            "google.protobuf.Value" => true,
            "google.protobuf.ListValue" => v.is_array(),
            "google.protobuf.StringValue" | "google.protobuf.BytesValue" => v.is_string(),
            "google.protobuf.BoolValue" => v.is_boolean(),
            _ if m.full_name().starts_with("google.protobuf.")
                && m.full_name().ends_with("Value") =>
            {
                m.get_field_by_name("value")
                    .is_some_and(|f| scalar_matches(v, &f.kind()))
            }
            _ => v.is_object(),
        },
        Kind::Enum(e) => {
            v.as_str().is_some_and(|n| e.get_value_by_name(n).is_some())
                || v.as_i64().is_some_and(|n| {
                    i32::try_from(n)
                        .ok()
                        .is_some_and(|n| e.get_value(n).is_some())
                })
        }
        _ => false,
    }
}
fn matches(v: &Value, field: &FieldDescriptor, nullable: bool, single: bool) -> bool {
    if v.is_null() {
        return nullable;
    }
    if !single && field.is_list() {
        v.is_array()
    } else if !single && field.is_map() {
        v.is_object()
    } else {
        scalar_matches(v, &leaf(field))
    }
}
fn enum_members(
    name: &str,
    pool: &DescriptorPool,
    catalog: &Value,
) -> Result<Vec<(Value, String, String)>> {
    if let Some(e) = catalog.get("enums").and_then(|m| m.get(name)) {
        let rows = e["field_list"]
            .as_array()
            .context("enum catalog needs field_list")?;
        return rows
            .iter()
            .map(|v| {
                ensure!(
                    v["field_value"].is_i64() || v["field_value"].is_u64(),
                    "enum field_value must be an integer"
                );
                Ok((
                    v["field_value"].clone(),
                    v["field_name"]
                        .as_str()
                        .context("enum field_name must be a string")?
                        .into(),
                    v["field_desc"].as_str().unwrap_or_default().into(),
                ))
            })
            .collect();
    }
    let e = pool.get_enum_by_name(name).with_context(|| {
        format!("unknown enum {name}; export the authoritative local enum catalog first")
    })?;
    Ok(e.values()
        .map(|v| (json!(v.number()), v.name().into(), String::new()))
        .collect())
}
pub fn build(pool: &DescriptorPool, source: &Path) -> Result<Fields> {
    let settings = crate::settings::Settings::load(source.parent().unwrap())?;
    let catalog_path = source.parent().unwrap().join(
        settings
            .enum_catalog
            .as_deref()
            .unwrap_or("enum_catalog.json"),
    );
    let catalog: Value = if catalog_path.exists() {
        serde_json::from_slice(&fs::read(catalog_path)?)?
    } else {
        json!({})
    };
    ensure!(
        catalog.as_object().is_some_and(|c| c.is_empty()) || catalog["version"] == 1,
        "unsupported enum_catalog version"
    );
    let mut templates = BTreeMap::new();
    let registry = pool
        .get_extension_by_name("common.doc_fields")
        .context("missing doc_fields extension")?;
    let extension = pool
        .get_extension_by_name("common.field")
        .context("missing field extension")?;
    for file in pool.files() {
        let opts = file.options();
        if opts.has_extension(&registry) {
            let data = options(opts.get_extension(&registry).into_owned())?;
            for item in data["templates"].as_array().into_iter().flatten() {
                let name = item["name"].as_str().context("template name required")?;
                ensure!(
                    !name.is_empty()
                        && templates
                            .insert(name.into(), item.get("field").cloned().unwrap_or(json!({})))
                            .is_none(),
                    "duplicate or empty template {name}"
                );
            }
        }
    }
    for (name, raw) in &templates {
        resolve(raw, &templates, &mut BTreeSet::from([name.clone()]))?;
    }
    let mut result = BTreeMap::new();
    for message in pool.all_messages() {
        if message.is_map_entry()
            || message.parent_file().name().starts_with("google/")
            || ["common/api_options.proto", "common/field_options.proto"]
                .contains(&message.parent_file().name())
        {
            continue;
        }
        for field in message.fields() {
            let opts = field.options();
            let raw = if opts.has_extension(&extension) {
                options(opts.get_extension(&extension).into_owned())?
            } else {
                json!({})
            };
            let mut meta = resolve(&raw, &templates, &mut BTreeSet::new())
                .with_context(|| format!("field {}", field.full_name()))?;
            for key in [
                "minUint",
                "maxUint",
                "minInt",
                "maxInt",
                "minLength",
                "maxLength",
                "minItems",
                "maxItems",
            ] {
                if let Some(s) = meta.get(key).and_then(Value::as_str) {
                    meta[key] = serde_json::from_str(s)?;
                }
            }
            ensure!(
                !comments(&field).contains("CFG="),
                "{}: migrate legacy CFG to common.field",
                field.full_name()
            );
            let model = make_model(&field, meta, pool, &catalog, false)
                .with_context(|| format!("field {}", field.full_name()))?;
            result.insert(field.full_name().into(), model);
        }
    }
    Ok(result)
}
fn parse_value(text: &str, field: &FieldDescriptor, single: bool) -> Result<Value> {
    let kind = leaf(field);
    let textual = matches!(kind, Kind::String | Kind::Bytes | Kind::Enum(_))
        || int64(&kind)
        || matches!(&kind, Kind::Message(m) if matches!(m.full_name(),"google.protobuf.Timestamp" | "google.protobuf.Duration" | "google.protobuf.StringValue" | "google.protobuf.BytesValue" | "google.protobuf.Int64Value" | "google.protobuf.UInt64Value"));
    if textual && (single || (!field.is_list() && !field.is_map())) {
        Ok(json!(text))
    } else {
        serde_json::from_str(text).context("invalid default/examples JSON text")
    }
}
fn make_model(
    field: &FieldDescriptor,
    meta: Value,
    pool: &DescriptorPool,
    catalog: &Value,
    single: bool,
) -> Result<FieldModel> {
    let mut model = FieldModel {
        description: meta
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| comments(field)),
        nullable: meta["nullable"] == true,
        optional: field.field_descriptor_proto().proto3_optional == Some(true)
            || field.is_list()
            || field.is_map()
            || field.containing_oneof().is_some(),
        options: meta.clone(),
        ..Default::default()
    };
    ensure!(
        !(meta["defaultNull"] == true && meta.get("default").is_some()),
        "default and default_null are mutually exclusive"
    );
    if meta["defaultNull"] == true {
        ensure!(model.nullable, "default_null requires nullable: true");
        model.default = Some(Value::Null);
    }
    if let Some(default) = meta.get("default").and_then(Value::as_str) {
        let v = parse_value(default, field, single)?;
        ensure!(
            matches(&v, field, model.nullable, single),
            "{} default has wrong type",
            field.full_name()
        );
        model.default = Some(v);
    }
    for text in meta["examples"].as_array().into_iter().flatten() {
        let v = parse_value(text.as_str().unwrap(), field, single)?;
        ensure!(
            matches(&v, field, model.nullable, single),
            "{} example has wrong type",
            field.full_name()
        );
        model.examples.push(v);
    }
    if meta["examplesNull"] == true {
        ensure!(model.nullable, "examples_null requires nullable");
        model.examples.push(Value::Null);
    }
    let kind = leaf(field);
    let number = meta.get("enumNumber").and_then(Value::as_str);
    let string = meta.get("enumStr").and_then(Value::as_str);
    if let Some(name) = number.or(string) {
        ensure!(
            if number.is_some() {
                integer(&kind)
            } else {
                matches!(kind, Kind::String)
            },
            "{}: enum_number requires integer, enum_str requires string",
            field.full_name()
        );
        let members = enum_members(name, pool, catalog)?;
        let mut values = vec![];
        let mut labels = vec![];
        for (value, name, desc) in members {
            let item = if number.is_some() {
                let (min, max) = integer_range(&kind).unwrap();
                ensure!(
                    integral(&value).is_some_and(|n| (min..=max).contains(&n)),
                    "enum member is outside field integer range"
                );
                value
            } else {
                if name.is_empty() {
                    continue;
                }
                json!(name)
            };
            if !values.contains(&item) {
                values.push(item.clone());
                labels.push(format!(
                    "{}: {}",
                    item.as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| item.to_string()),
                    desc
                ));
            }
        }
        ensure!(
            !values.is_empty(),
            "{} enum has no usable members",
            field.full_name()
        );
        if !labels.is_empty() {
            model.description = format!(
                "{}{}{}",
                model.description,
                if model.description.is_empty() {
                    ""
                } else {
                    "\n"
                },
                labels.join("; ")
            );
        }
        model.enum_values = Some(values);
        model.enum_number = number.is_some();
    }
    if let Some(items) = meta.get("enumJson").and_then(Value::as_array) {
        ensure!(
            model.enum_values.is_none(),
            "enum_json cannot be combined with an enum reference"
        );
        let mut values = vec![];
        for item in items {
            let v = serde_json::from_str(item.as_str().unwrap())?;
            ensure!(
                scalar_matches(&v, &kind),
                "{} enum_json has wrong type",
                field.full_name()
            );
            values.push(v);
        }
        model.enum_number = integer(&kind);
        model.enum_values = Some(values);
    }
    if meta["upload"] == true {
        ensure!(
            matches!(kind, Kind::String | Kind::Bytes),
            "{} upload must be string or bytes",
            field.full_name()
        );
    }
    if let Some(pattern) = meta.get("pattern").and_then(Value::as_str) {
        ensure!(matches!(kind, Kind::String), "pattern requires string");
        regex::Regex::new(pattern)?;
    }
    if let Some(items) = meta.get("items") {
        ensure!(
            !single && (field.is_list() || field.is_map()),
            "items metadata requires a list or map"
        );
        let mut item = items.clone();
        for key in [
            "minUint",
            "maxUint",
            "minInt",
            "maxInt",
            "minLength",
            "maxLength",
            "minItems",
            "maxItems",
        ] {
            if let Some(s) = item.get(key).and_then(Value::as_str) {
                item[key] = serde_json::from_str(s)?;
            }
        }
        model.items = Some(Box::new(make_model(field, item, pool, catalog, true)?));
    }
    ensure!(
        !(model.enum_values.is_some()
            && model
                .items
                .as_ref()
                .is_some_and(|i| i.enum_values.is_some())),
        "declare enum on the container or items, not both"
    );
    let collection = !single && (field.is_list() || field.is_map());
    for (keys, valid) in [
        (
            &["minUint", "maxUint"][..],
            !collection
                && matches!(
                    kind,
                    Kind::Uint32 | Kind::Fixed32 | Kind::Uint64 | Kind::Fixed64
                ),
        ),
        (
            &["minInt", "maxInt"][..],
            !collection
                && matches!(
                    kind,
                    Kind::Int32
                        | Kind::Sint32
                        | Kind::Sfixed32
                        | Kind::Int64
                        | Kind::Sint64
                        | Kind::Sfixed64
                ),
        ),
        (
            &["minimum", "maximum"][..],
            !collection && matches!(kind, Kind::Float | Kind::Double),
        ),
        (
            &["minLength", "maxLength", "pattern", "format"][..],
            !collection && matches!(kind, Kind::String | Kind::Bytes),
        ),
        (&["minItems", "maxItems"][..], !single && field.is_list()),
    ] {
        ensure!(valid || keys.iter().all(|k| meta.get(*k).is_none()), "constraint incompatible with Proto type; collection element constraints belong in items");
    }
    for (low, high) in [
        ("minUint", "maxUint"),
        ("minInt", "maxInt"),
        ("minimum", "maximum"),
        ("minLength", "maxLength"),
        ("minItems", "maxItems"),
    ] {
        if let (Some(a), Some(b)) = (meta.get(low), meta.get(high)) {
            ensure!(
                if low == "minimum" {
                    a.as_f64().unwrap() <= b.as_f64().unwrap()
                } else {
                    integral(a).unwrap() <= integral(b).unwrap()
                },
                "minimum exceeds maximum"
            );
        }
    }
    if let Some((min, max)) = integer_range(&kind) {
        for key in ["minUint", "maxUint", "minInt", "maxInt"] {
            if let Some(v) = meta.get(key) {
                ensure!(
                    integral(v).is_some_and(|n| (min..=max).contains(&n)),
                    "integer bound outside Proto range"
                );
            }
        }
    }
    for v in model.default.iter().chain(model.examples.iter()) {
        validate_value(field, &model, v, single).with_context(|| {
            format!("{} default/example violates constraints", field.full_name())
        })?;
    }
    Ok(model)
}
fn validate_value(
    field: &FieldDescriptor,
    model: &FieldModel,
    v: &Value,
    single: bool,
) -> Result<()> {
    if v.is_null() {
        ensure!(model.nullable, "null is not allowed");
        return Ok(());
    }
    let values: Vec<&Value> = if !single && field.is_list() {
        v.as_array().unwrap().iter().collect()
    } else if !single && field.is_map() {
        v.as_object().unwrap().values().collect()
    } else {
        vec![v]
    };
    for v in values {
        if !single && (field.is_list() || field.is_map()) {
            if let Some(item) = &model.items {
                validate_value(field, item, v, true)?;
            } else {
                ensure!(
                    scalar_matches(v, &leaf(field)),
                    "collection element has wrong type"
                );
            }
        }
        if let Some(allowed) = &model.enum_values {
            ensure!(
                allowed.iter().any(|a| if model.enum_number {
                    json_number(a) == json_number(v)
                } else {
                    a == v
                }),
                "value is outside enum"
            );
        }
        for (key, is_min) in [
            ("minimum", true),
            ("maximum", false),
            ("minUint", true),
            ("maxUint", false),
            ("minInt", true),
            ("maxInt", false),
        ] {
            if let Some(bound) = model.options.get(key) {
                let valid = if matches!(key, "minimum" | "maximum") {
                    let n = v.as_f64().context("numeric constraint on non-number")?;
                    let b = bound.as_f64().context("invalid bound")?;
                    if is_min {
                        n >= b
                    } else {
                        n <= b
                    }
                } else {
                    let n = integral(v).context("integer constraint on non-integer")?;
                    let b = integral(bound).context("invalid integer bound")?;
                    if is_min {
                        n >= b
                    } else {
                        n <= b
                    }
                };
                ensure!(valid, "value outside bound");
            }
        }
        if let Some(p) = model.options.get("pattern").and_then(Value::as_str) {
            ensure!(
                regex::Regex::new(p)?.is_match(v.as_str().context("pattern needs string")?),
                "value violates pattern"
            );
        }
    }
    for (min, max, len) in [
        ("minItems", "maxItems", v.as_array().map(Vec::len)),
        (
            "minLength",
            "maxLength",
            v.as_str().map(|s| s.chars().count()),
        ),
    ] {
        if let Some(len) = len {
            if let Some(n) = model.options.get(min).and_then(Value::as_u64) {
                ensure!(len as u64 >= n, "too short");
            }
            if let Some(n) = model.options.get(max).and_then(Value::as_u64) {
                ensure!(len as u64 <= n, "too long");
            }
        }
    }
    Ok(())
}

pub fn rules(pool: &DescriptorPool, models: &Fields) -> Value {
    let mut active: BTreeSet<String> = pool
        .all_messages()
        .filter(|m| {
            m.fields().any(|f| {
                models.get(f.full_name()).is_some_and(|x| {
                    x.enum_values.is_some()
                        || x.items.as_ref().is_some_and(|i| i.enum_values.is_some())
                })
            })
        })
        .map(|m| m.full_name().into())
        .collect();
    loop {
        let old = active.len();
        for m in pool.all_messages() {
            if m.fields().any(
                |f| matches!(leaf(&f),Kind::Message(child)if active.contains(child.full_name())),
            ) {
                active.insert(m.full_name().into());
            }
        }
        if old == active.len() {
            break;
        }
    }
    let mut messages = Map::new();
    for m in pool.all_messages() {
        if !active.contains(m.full_name()) {
            continue;
        }
        let mut fields = Map::new();
        for f in m.fields() {
            let Some(model) = models.get(f.full_name()) else {
                continue;
            };
            let mut rule = json!({"optional":model.optional,"nullable":model.nullable,"collection":if f.is_map(){"map"}else if f.is_list(){"list"}else{"one"}});
            rule["item_nullable"] = json!(model.items.as_ref().is_some_and(|i| i.nullable));
            let enum_model = model
                .items
                .as_deref()
                .filter(|i| i.enum_values.is_some())
                .unwrap_or(model);
            if let Some(values) = &enum_model.enum_values {
                rule["kind"] = json!(if enum_model.enum_number {
                    "number"
                } else {
                    "literal"
                });
                rule["values"] = if enum_model.enum_number {
                    json!(values
                        .iter()
                        .map(|v| json_number(v).unwrap())
                        .collect::<Vec<_>>())
                } else {
                    json!(values)
                };
            } else if let Kind::Message(child) = leaf(&f) {
                if !active.contains(child.full_name()) {
                    continue;
                }
                rule["kind"] = json!("message");
                rule["message"] = json!(child.full_name());
            } else {
                continue;
            }
            fields.insert(f.name().into(), rule);
        }
        messages.insert(m.full_name().into(), json!(fields));
    }
    json!({"version":1,"messages":messages})
}
