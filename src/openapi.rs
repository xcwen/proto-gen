use crate::{
    compiler::{Endpoint, Workspace},
    field_model::FieldModel,
    settings::Project,
};
use anyhow::{bail, ensure, Context, Result};
use prost_reflect::{FieldDescriptor, Kind, MessageDescriptor};
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;

pub struct Builder<'a> {
    pub workspace: &'a Workspace,
    pub components: Map<String, Value>,
    output: bool,
}
impl<'a> Builder<'a> {
    pub fn new(workspace: &'a Workspace, output: bool) -> Self {
        Self {
            workspace,
            components: Map::new(),
            output,
        }
    }
    fn key(&self, m: &MessageDescriptor) -> String {
        format!(
            "{}.{}",
            m.full_name(),
            if self.output { "response" } else { "request" }
        )
    }
    pub fn message(&mut self, m: MessageDescriptor) -> Result<Value> {
        let key = self.key(&m);
        let reference = json!({"$ref":format!("#/components/schemas/{key}")});
        if self.components.contains_key(&key) {
            return Ok(reference);
        }
        self.components.insert(key.clone(), json!({}));
        let schema = self.inline(m)?;
        self.components.insert(key, schema);
        Ok(reference)
    }
    fn kind(&mut self, kind: Kind) -> Result<Value> {
        Ok(match kind {
            Kind::Message(m) => match m.full_name() {
                "google.protobuf.Timestamp" => json!({"type":"string","format":"date-time"}),
                "google.protobuf.Duration" => {
                    json!({"type":"string","pattern":"^-?[0-9]+(\\.[0-9]{1,9})?s$"})
                }
                "google.protobuf.Value" => json!({}),
                "google.protobuf.Struct" => json!({"type":"object"}),
                "google.protobuf.ListValue" => json!({"type":"array","items":{}}),
                "google.protobuf.Any" => {
                    json!({"type":"object","properties":{"@type":{"type":"string"}},"required":["@type"],"additionalProperties":true})
                }
                _ if m.full_name().ends_with("Value")
                    && m.full_name().starts_with("google.protobuf.") =>
                {
                    self.kind(
                        m.get_field_by_name("value")
                            .context("wrapper missing value")?
                            .kind(),
                    )?
                }
                _ => self.message(m)?,
            },
            Kind::Bool => json!({"type":"boolean"}),
            Kind::String => json!({"type":"string"}),
            Kind::Bytes => json!({"type":"string","format":"byte","contentEncoding":"base64"}),
            Kind::Float => json!({"type":"number","format":"float"}),
            Kind::Double => json!({"type":"number","format":"double"}),
            Kind::Int64 | Kind::Sint64 | Kind::Sfixed64 => {
                json!({"type":"string","format":"int64","pattern":"^-?(0|[1-9][0-9]*)$"})
            }
            Kind::Uint64 | Kind::Fixed64 => {
                json!({"type":"string","format":"uint64","pattern":"^(0|[1-9][0-9]*)$"})
            }
            Kind::Uint32 | Kind::Fixed32 => {
                json!({"type":"integer","minimum":0,"maximum":4294967295u64})
            }
            Kind::Int32 | Kind::Sint32 | Kind::Sfixed32 => {
                json!({"type":"integer","format":"int32","minimum":-2147483648i64,"maximum":2147483647})
            }
            Kind::Enum(e) if e.full_name() == "google.protobuf.NullValue" => json!({"type":"null"}),
            Kind::Enum(e) => {
                if self.output && self.workspace.target == "typescript" {
                    json!({"type":"integer","enum":e.values().map(|v|v.number()).collect::<Vec<_>>()})
                } else {
                    json!({"type":"string","enum":e.values().map(|v|v.name().to_owned()).collect::<Vec<_>>()})
                }
            }
        })
    }
    fn annotations(&self, mut schema: Value, model: &FieldModel, kind: &Kind) -> Result<Value> {
        if !model.description.is_empty() {
            schema["description"] = json!(model.description);
        }
        if let Some(v) = &model.default {
            schema["default"] = v.clone();
        }
        if !model.examples.is_empty() {
            schema["examples"] = json!(model.examples);
        }
        if let Some(values) = &model.enum_values {
            schema["enum"] = json!(values
                .iter()
                .map(|v| if crate::field_model::int64(kind) && v.is_number() {
                    json!(v.to_string())
                } else {
                    v.clone()
                })
                .collect::<Vec<_>>());
        }
        if model.options["upload"] == true {
            schema["type"] = json!("string");
            schema["format"] = json!("binary");
            schema.as_object_mut().unwrap().remove("contentEncoding");
        }
        if let Some(v) = model.options.get("format") {
            schema["format"] = v.clone();
        }
        for (from, to) in [
            ("minUint", "minimum"),
            ("maxUint", "maximum"),
            ("minInt", "minimum"),
            ("maxInt", "maximum"),
            ("minimum", "minimum"),
            ("maximum", "maximum"),
            ("minLength", "minLength"),
            ("maxLength", "maxLength"),
            ("minItems", "minItems"),
            ("maxItems", "maxItems"),
            ("pattern", "pattern"),
        ] {
            if let Some(v) = model.options.get(from) {
                if crate::field_model::int64(kind) && ["minimum", "maximum"].contains(&to) {
                    schema[format!("x-proto-{to}")] = json!(v.to_string());
                } else {
                    schema[to] = v.clone();
                }
            }
        }
        if model.nullable {
            if let Some(t) = schema
                .get("type")
                .and_then(Value::as_str)
                .map(str::to_owned)
            {
                schema["type"] = json!([t, "null"]);
            } else {
                let mut annotations = Map::new();
                for key in ["description", "default", "examples"] {
                    if let Some(v) = schema.as_object_mut().unwrap().remove(key) {
                        annotations.insert(key.into(), v);
                    }
                }
                schema = json!({"anyOf":[schema,{"type":"null"}]});
                schema.as_object_mut().unwrap().extend(annotations);
            }
            if let Some(xs) = schema.get_mut("enum").and_then(Value::as_array_mut) {
                if !xs.contains(&Value::Null) {
                    xs.push(Value::Null);
                }
            }
        }
        Ok(schema)
    }
    fn field(&mut self, field: &FieldDescriptor) -> Result<Value> {
        let model = self
            .workspace
            .fields
            .get(field.full_name())
            .context("missing field metadata")?
            .clone();
        let kind = if field.is_map() {
            let Kind::Message(m) = field.kind() else {
                unreachable!()
            };
            m.get_field_by_name("value").unwrap().kind()
        } else {
            field.kind()
        };
        let mut item = self.kind(kind.clone())?;
        if field.is_list() || field.is_map() {
            let mut item_model = model.items.as_deref().cloned().unwrap_or_default();
            if model.enum_values.is_some() {
                item_model.enum_values = model.enum_values.clone();
                item_model.enum_number = model.enum_number;
            }
            if model.options["upload"] == true {
                item_model.options = json!({"upload":true});
            }
            item = self.annotations(item, &item_model, &kind)?;
            let mut outer = model.clone();
            outer.enum_values = None;
            outer.options.as_object_mut().unwrap().remove("upload");
            let schema = if field.is_list() {
                json!({"type":"array","items":item})
            } else {
                json!({"type":"object","additionalProperties":item})
            };
            self.annotations(schema, &outer, &kind)
        } else {
            self.annotations(item, &model, &kind)
        }
    }
    pub fn inline(&mut self, m: MessageDescriptor) -> Result<Value> {
        let mut props = Map::new();
        let mut required = vec![];
        for f in m.fields() {
            let model = &self.workspace.fields[f.full_name()];
            if !model.optional && !(self.output && f.supports_presence()) {
                required.push(f.name().to_owned());
            }
            props.insert(f.name().into(), self.field(&f)?);
        }
        let mut schema = json!({"type":"object","properties":props});
        if !required.is_empty() {
            schema["required"] = json!(required);
        }
        let mut groups = vec![];
        for group in m.oneofs().filter(|g| !g.is_synthetic()) {
            let names: Vec<_> = group.fields().map(|f| f.name().to_owned()).collect();
            let mut variants = vec![];
            for name in &names {
                let others: Vec<_> = names
                    .iter()
                    .filter(|n| *n != name)
                    .map(|n| json!({"required":[n]}))
                    .collect();
                let mut branch = json!({"required":[name]});
                if !others.is_empty() {
                    branch["not"] = json!({"anyOf":others});
                }
                variants.push(branch);
            }
            variants.push(json!({"not":{"anyOf":names.iter().map(|n|json!({"required":[n]})).collect::<Vec<_>>()}}));
            groups.push(json!({"oneOf":variants}));
        }
        if !groups.is_empty() {
            schema["allOf"] = json!(groups);
        }
        Ok(schema)
    }
    pub fn input(&mut self, a: &Endpoint) -> Result<Value> {
        if let Some(m) = self
            .workspace
            .pool
            .get_message_by_name(&format!("{}.apidoc_in", a.package))
        {
            let groups: Vec<_> = m.oneofs().filter(|o| !o.is_synthetic()).collect();
            ensure!(
                groups.len() == 1 && m.fields().all(|f| f.containing_oneof().is_some()),
                "apidoc_in needs exactly one oneof"
            );
            let group = &groups[0];
            let mut branches = vec![];
            for f in group.fields() {
                let Kind::Message(child) = f.kind() else {
                    bail!("apidoc_in arms must be messages")
                };
                let mut branch = self.inline(child)?;
                ensure!(
                    branch["properties"].get(group.name()).is_none(),
                    "discriminator conflicts with a field"
                );
                let mut properties = Map::new();
                properties.insert(
                    group.name().into(),
                    json!({"type":"string","const":f.name()}),
                );
                properties.append(branch["properties"].as_object_mut().unwrap());
                branch["x-apifox-orders"] = json!(properties.keys().collect::<Vec<_>>());
                branch["properties"] = Value::Object(properties);
                if branch.get("required").is_none() {
                    branch["required"] = json!([]);
                }
                branch["required"]
                    .as_array_mut()
                    .unwrap()
                    .insert(0, json!(group.name()));
                branches.push(branch);
            }
            return Ok(json!({"oneOf":branches}));
        }
        self.inline(
            self.workspace
                .pool
                .get_message_by_name(&format!("{}.in", a.package))
                .context("missing input type")?,
        )
    }
}
fn folder(path: &[String]) -> String {
    if path.is_empty() {
        return "开发中".into();
    }
    path.iter()
        .map(|s| s.replace('\\', "\\\\").replace('/', "\\/"))
        .collect::<Vec<_>>()
        .join("/")
}
pub fn document(a: &Endpoint, w: &Workspace, project: &Project) -> Result<Value> {
    let mut input = Builder::new(w, false);
    let input_schema = input.input(a)?;
    let mut output = Builder::new(w, true);
    let business = output.message(
        w.pool
            .get_message_by_name(&format!("{}.out", a.package))
            .context("missing output type")?,
    )?;
    let mode = a.meta.docs.response_mode.as_str();
    let mut response_schema = match mode {
        "RAW_JSON" => business.clone(),
        "SSE" => json!({"type":"string","description":"Server-sent events"}),
        "BINARY" => json!({"type":"string","format":"binary"}),
        "REDIRECT" => json!({}),
        _ => {
            let message = if a.meta.docs.message_field.is_empty() {
                "info"
            } else {
                &a.meta.docs.message_field
            };
            let mut props = json!({"code":{"type":"integer","description":"业务错误码，0 表示成功"},"request_id":{"type":"string"},"data":business});
            props[message] = json!({"type":"string"});
            json!({"type":"object","properties":props,"required":["code",message,"request_id"]})
        }
    };
    if !a.meta.docs.response_schema_json.is_empty() {
        response_schema = serde_json::from_str(&a.meta.docs.response_schema_json)?;
        ensure!(
            response_schema.is_object() || response_schema.is_boolean(),
            "response_schema_json must be a JSON Schema"
        );
    }
    let response_type = if !a.meta.docs.response_content_type.is_empty() {
        a.meta.docs.response_content_type.clone()
    } else {
        match mode {
            "SSE" => "text/event-stream",
            "BINARY" => "application/octet-stream",
            _ => "application/json",
        }
        .into()
    };
    let code = if mode == "REDIRECT" { "302" } else { "200" };
    let mut response = json!({"description":if mode=="REDIRECT"{"Redirect"}else{"成功"}});
    if mode == "REDIRECT" {
        response["headers"] = json!({"Location":{"schema":{"type":"string","format":"uri"}}});
    } else {
        response["content"] = json!({response_type.clone():{"schema":response_schema.clone()}});
    }
    let status = if a.meta.deprecated {
        "deprecated"
    } else if a.meta.docs.status.is_empty() {
        "released"
    } else {
        &a.meta.docs.status
    };
    ensure!(
        [
            "designing",
            "pending",
            "developing",
            "integrating",
            "testing",
            "tested",
            "released",
            "deprecated",
            "exception",
            "obsolete"
        ]
        .contains(&status),
        "invalid API document status"
    );
    let description = if let Some(reason) = &a.meta.deprecation_message {
        format!("{}\n\n弃用说明：{}", a.meta.description, reason)
    } else {
        a.meta.description.clone()
    };
    let mut op = json!({"operationId":a.name(),"summary":a.meta.title,"description":description,"x-apifox-folder":folder(&a.meta.category_path),"x-apifox-status":status,"responses":{code:response}});
    if a.meta.deprecated {
        op["deprecated"] = json!(true);
    }
    if let Some(owner) = a.meta.maintainers.first() {
        op["x-apifox-maintainer"] = json!(owner);
        op["x-maintainers"] = json!(a.meta.maintainers);
    }
    if a.meta.docs.auth == "NO_AUTH" {
        op["security"] = json!([]);
    }
    if project.export_tags_flag != Some(false) {
        let mut tags = a.meta.tags.clone();
        if !a.meta.project.is_empty() {
            tags.push(format!("PROJECT:{}", a.meta.project));
        }
        tags.push(if w.target == "typescript" {
            "node".into()
        } else {
            w.target.clone()
        });
        let mut seen = BTreeSet::new();
        tags.retain(|v| seen.insert(v.clone()));
        op["tags"] = json!(tags);
    }
    let method = if a.method().is_empty() {
        "POST"
    } else {
        a.method()
    };
    let request_type = a
        .meta
        .http
        .as_ref()
        .and_then(|h| h.request_content_type.as_ref())
        .filter(|s| !s.is_empty())
        .map(String::as_str)
        .unwrap_or("application/json");
    ensure!(
        !(input_schema.get("oneOf").is_some() && matches!(method, "GET" | "HEAD")),
        "GET/HEAD with apidoc_in branches requires an explicit query contract"
    );
    let mut body_schema = input_schema.clone();
    let mut parameters = vec![];
    if let Some(properties) = input_schema.get("properties").and_then(Value::as_object) {
        let message = w
            .pool
            .get_message_by_name(&format!("{}.in", a.package))
            .unwrap();
        let mut body = Map::new();
        let mut required = vec![];
        for (name, schema) in properties {
            let field = message.get_field_by_name(name).unwrap();
            let meta = &w.fields[field.full_name()];
            let place = meta.options["location"]
                .as_str()
                .filter(|s| *s != "INHERIT")
                .unwrap_or(if ["GET", "HEAD"].contains(&method) {
                    "QUERY"
                } else {
                    "BODY"
                });
            if place == "BODY" {
                ensure!(
                    !["GET", "HEAD"].contains(&method),
                    "GET/HEAD fields must use query parameters"
                );
                body.insert(name.clone(), schema.clone());
                if !meta.optional {
                    required.push(name.clone());
                }
            } else {
                let location = place.to_lowercase();
                let name = meta.options["httpName"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .unwrap_or(name);
                ensure!(
                    !matches!(location.as_str(), "header")
                        || !["authorization", "tenant-id"].contains(&name.to_lowercase().as_str()),
                    "authentication headers belong to project Auth configuration"
                );
                ensure!(
                    location != "path" || !meta.optional,
                    "path parameters cannot be optional"
                );
                let mut parameter = json!({"name":name,"in":location,"required":!meta.optional});
                if schema["type"] == "array"
                    || schema["type"] == "object"
                    || schema.get("$ref").is_some()
                {
                    parameter["content"] = json!({"application/json":{"schema":schema}});
                } else {
                    parameter["schema"] = schema.clone();
                }
                parameters.push(parameter);
            }
        }
        body_schema = input_schema.clone();
        body_schema["properties"] = json!(body);
        body_schema.as_object_mut().unwrap().remove("required");
        if !required.is_empty() {
            body_schema["required"] = json!(required);
        }
    }
    ensure!(
        parameters.is_empty() || input_schema.get("allOf").is_none(),
        "oneof across HTTP parameters/body is not supported; use a body contract"
    );
    if !parameters.is_empty() {
        op["parameters"] = json!(parameters);
    }
    let has_body = !matches!(method, "GET" | "HEAD")
        && (body_schema.get("oneOf").is_some()
            || body_schema["properties"]
                .as_object()
                .is_some_and(|m| !m.is_empty()));
    if has_body {
        op["requestBody"] = json!({"required":body_schema.get("oneOf").is_some() || body_schema["required"].as_array().is_some_and(|r|!r.is_empty()),"content":{request_type:{"schema":body_schema.clone()}}});
    }
    let mut components = input.components;
    components.extend(output.components);
    check_schema(&body_schema, &components)?;
    check_schema(&response_schema, &components)?;
    for schema in components.values() {
        check_schema(schema, &components)?;
    }
    let mut request_examples = Map::new();
    let mut response_examples = Map::new();
    for (i, e) in a.meta.examples.iter().enumerate() {
        let name = e.name.clone().unwrap_or_else(|| (i + 1).to_string());
        let summary = e.summary.as_ref().unwrap_or(&name);
        if let Some(data) = &e.request_json {
            let value = serde_json::from_str(data)?;
            validate_instance(&body_schema, &value, &components)
                .with_context(|| format!("{} request example {name}", a.source))?;
            ensure!(
                request_examples
                    .insert(name.clone(), json!({"summary":summary,"value":value}))
                    .is_none(),
                "duplicate request example"
            );
        }
        if e.response_json.is_some() || e.data_json.is_some() {
            let value = if let Some(data) = &e.response_json {
                serde_json::from_str(data)?
            } else {
                ensure!(
                    mode.is_empty() || mode == "STANDARD_JSON",
                    "data_json requires standard JSON response"
                );
                let message = if a.meta.docs.message_field.is_empty() {
                    "info"
                } else {
                    &a.meta.docs.message_field
                };
                let mut value = json!({"code":0,"request_id":"0123456789abcdef0123456789abcdef","data":serde_json::from_str::<Value>(e.data_json.as_ref().unwrap())?});
                value[message] = json!("succ");
                value
            };
            ensure!(
                response_type == "application/json",
                "JSON response examples require a JSON response"
            );
            validate_instance(&response_schema, &value, &components)
                .with_context(|| format!("{} response example {name}", a.source))?;
            ensure!(
                response_examples
                    .insert(name.clone(), json!({"summary":summary,"value":value}))
                    .is_none(),
                "duplicate response example"
            );
        }
    }
    if !request_examples.is_empty() {
        ensure!(
            has_body,
            "body examples cannot be attached to GET/HEAD or empty body"
        );
        op["requestBody"]["content"][request_type]["examples"] = json!(request_examples);
    }
    if !response_examples.is_empty() {
        op["responses"][code]["content"][&response_type]["examples"] = json!(response_examples);
    }
    let path = crate::apifox::api_path(&a.path, &project.url_replace_cfg)?;
    let placeholders: BTreeSet<String> = regex::Regex::new(r"\{([^{}]+)\}")?
        .captures_iter(&path)
        .map(|c| c[1].to_owned())
        .collect();
    let declared: BTreeSet<String> = op["parameters"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|p| p["in"] == "path")
        .map(|p| p["name"].as_str().unwrap().to_owned())
        .collect();
    ensure!(
        placeholders == declared,
        "path placeholders and path parameters must match"
    );
    let doc = json!({"openapi":"3.1.0","info":{"title":project.apifox_project_name,"version":"1"},"paths":{path:{method.to_lowercase():op}},"components":{"schemas":components}});
    validate_document(&doc)?;
    Ok(doc)
}

pub fn validate_instance(
    schema: &Value,
    value: &Value,
    components: &Map<String, Value>,
) -> Result<()> {
    let mut root = schema.clone();
    if root.is_object() {
        root["components"] = json!({"schemas":components});
    }
    let validator = jsonschema::draft202012::options()
        .should_validate_formats(true)
        .with_format("int64", |s| s.parse::<i64>().is_ok())
        .with_format("uint64", |s| s.parse::<u64>().is_ok())
        .build(&root)
        .map_err(|e| anyhow::anyhow!("invalid JSON Schema: {e}"))?;
    validator.validate(value).map_err(|e| {
        anyhow::anyhow!(
            "example/default at {} violates schema: {}",
            e.instance_path(),
            e.schema_path()
        )
    })?;
    validate(schema, value, components, 0)
}
fn check_schema(schema: &Value, components: &Map<String, Value>) -> Result<()> {
    jsonschema::draft202012::meta::validate(schema)
        .map_err(|e| anyhow::anyhow!("invalid JSON Schema: {e}"))?;
    fn annotations(s: &Value, c: &Map<String, Value>) -> Result<()> {
        if let Some(v) = s.get("default") {
            validate_instance(s, v, c).context("invalid field default")?;
        }
        for v in s["examples"].as_array().into_iter().flatten() {
            validate_instance(s, v, c).context("invalid field example")?;
        }
        for key in ["properties", "$defs"] {
            for v in s[key].as_object().into_iter().flat_map(|x| x.values()) {
                annotations(v, c)?;
            }
        }
        for key in ["items", "additionalProperties", "not"] {
            if let Some(v) = s.get(key) {
                annotations(v, c)?;
            }
        }
        for key in ["oneOf", "anyOf", "allOf"] {
            for v in s[key].as_array().into_iter().flatten() {
                annotations(v, c)?;
            }
        }
        Ok(())
    }
    annotations(schema, components)
}
fn validate(s: &Value, v: &Value, c: &Map<String, Value>, depth: usize) -> Result<()> {
    ensure!(depth < 128, "JSON example is too deep");
    if s == &json!(true) {
        return Ok(());
    }
    ensure!(s != &json!(false), "value rejected by schema");
    if let Some(reference) = s.get("$ref").and_then(Value::as_str) {
        let key = reference
            .strip_prefix("#/components/schemas/")
            .context("only local component references are supported")?;
        validate(
            c.get(key)
                .with_context(|| format!("unknown schema {key}"))?,
            v,
            c,
            depth + 1,
        )?;
    }
    if let Some(items) = s.get("allOf").and_then(Value::as_array) {
        for item in items {
            validate(item, v, c, depth + 1)?;
        }
    }
    for (key, exact) in [("oneOf", true), ("anyOf", false)] {
        if let Some(xs) = s.get(key).and_then(Value::as_array) {
            let n = xs
                .iter()
                .filter(|x| validate(x, v, c, depth + 1).is_ok())
                .count();
            ensure!(
                if exact { n == 1 } else { n > 0 },
                "value does not match {key}"
            );
        }
    }
    if let Some(no) = s.get("not") {
        ensure!(
            validate(no, v, c, depth + 1).is_err(),
            "value matches excluded schema"
        );
    }
    if let Some(t) = s.get("type") {
        let valid = |t: &str| match t {
            "string" => v.is_string(),
            "integer" => v.is_i64() || v.is_u64(),
            "number" => v.is_number(),
            "boolean" => v.is_boolean(),
            "object" => v.is_object(),
            "array" => v.is_array(),
            "null" => v.is_null(),
            _ => false,
        };
        ensure!(
            t.as_str().map(valid).unwrap_or_else(|| t
                .as_array()
                .is_some_and(|a| a.iter().any(|x| x.as_str().is_some_and(valid)))),
            "wrong JSON type"
        );
    }
    if let Some(x) = s.get("const") {
        ensure!(v == x, "value differs from const");
    }
    if let Some(xs) = s.get("enum").and_then(Value::as_array) {
        ensure!(xs.contains(v), "value is outside enum");
    }
    if let Some(object) = v.as_object() {
        for name in s["required"].as_array().into_iter().flatten() {
            ensure!(
                object.contains_key(name.as_str().unwrap()),
                "missing field {}",
                name
            );
        }
        for (k, x) in object {
            if let Some(p) = s.get("properties").and_then(|p| p.get(k)) {
                validate(p, x, c, depth + 1).with_context(|| format!("property {k}"))?;
            } else if let Some(p) = s.get("additionalProperties") {
                validate(p, x, c, depth + 1)?;
            }
        }
    }
    if let Some(xs) = v.as_array() {
        for (min, is_min) in [("minItems", true), ("maxItems", false)] {
            if let Some(n) = s.get(min).and_then(Value::as_u64) {
                ensure!(
                    if is_min {
                        xs.len() as u64 >= n
                    } else {
                        xs.len() as u64 <= n
                    },
                    "array length outside bound"
                );
            }
        }
        if let Some(item) = s.get("items") {
            for x in xs {
                validate(item, x, c, depth + 1)?;
            }
        }
    }
    if let Some(text) = v.as_str() {
        for (key, is_min) in [("minLength", true), ("maxLength", false)] {
            if let Some(n) = s.get(key).and_then(Value::as_u64) {
                ensure!(
                    if is_min {
                        text.chars().count() as u64 >= n
                    } else {
                        text.chars().count() as u64 <= n
                    },
                    "string length outside bound"
                );
            }
        }
        if let Some(p) = s.get("pattern").and_then(Value::as_str) {
            ensure!(
                regex::Regex::new(p)?.is_match(text),
                "string violates pattern"
            );
        }
        if s["format"] == "int64" {
            text.parse::<i64>().context("int64 string outside range")?;
        }
        if s["format"] == "uint64" {
            text.parse::<u64>().context("uint64 string outside range")?;
        }
        for (key, is_min) in [("x-proto-minimum", true), ("x-proto-maximum", false)] {
            if let Some(bound) = s.get(key).and_then(Value::as_str) {
                let n = text.parse::<i128>()?;
                let b = bound.parse::<i128>()?;
                ensure!(
                    if is_min { n >= b } else { n <= b },
                    "64-bit string outside bound"
                );
            }
        }
    }
    if let Some(n) = v.as_f64() {
        if let Some(min) = s.get("minimum").and_then(Value::as_f64) {
            ensure!(n >= min, "number below minimum");
        }
        if let Some(max) = s.get("maximum").and_then(Value::as_f64) {
            ensure!(n <= max, "number above maximum");
        }
    }
    Ok(())
}

/// Validate against the vendored official OpenAPI 3.1 schema, without network retrieval.
pub fn validate_document(doc: &Value) -> Result<()> {
    static VALIDATOR: std::sync::OnceLock<jsonschema::Validator> = std::sync::OnceLock::new();
    let validator = VALIDATOR.get_or_init(|| {
        let schema: Value =
            serde_json::from_str(include_str!("../include/openapi-3.1.schema.json"))
                .expect("vendored OpenAPI schema");
        jsonschema::draft202012::options()
            .build(&schema)
            .expect("valid official OpenAPI schema")
    });
    validator.validate(doc).map_err(|e| {
        anyhow::anyhow!(
            "invalid OpenAPI document at {}: {}",
            e.instance_path(),
            e.schema_path()
        )
    })
}
