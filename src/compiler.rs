use anyhow::{bail, ensure, Context, Result};
use prost_reflect::DescriptorPool;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
};

pub const SCHEMA: &str = include_str!("../schema/common/api_options.proto");
const DESCRIPTOR_SCHEMA: &str = include_str!("../include/google/protobuf/descriptor.proto");

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub version: u32,
    pub target: String,
    pub source: PathBuf,
    pub output: PathBuf,
    pub controllers: PathBuf,
    pub registry: Option<PathBuf>,
    pub profile: Option<String>,
}
impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let mut value: Self = toml::from_str(&fs::read_to_string(path)?)?;
        ensure!(value.version == 1, "unsupported configuration version");
        ensure!(
            ["php", "typescript", "rust", "go"].contains(&value.target.as_str()),
            "unknown target"
        );
        let base = path.canonicalize()?.parent().unwrap().to_owned();
        for p in [&mut value.source, &mut value.output, &mut value.controllers] {
            *p = base.join(&*p);
        }
        value.registry = value.registry.map(|p| base.join(p));
        Ok(value)
    }
    pub fn root(&self) -> PathBuf {
        self.source.parent().unwrap().parent().unwrap().to_owned()
    }
    pub fn php(&self) -> bool {
        self.target == "php"
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Http {
    #[serde(default)]
    pub method: String,
    pub path: Option<String>,
    pub request_content_type: Option<String>,
}
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Example {
    pub name: Option<String>,
    pub request_json: Option<String>,
    pub response_json: Option<String>,
    pub summary: Option<String>,
    pub data_json: Option<String>,
}
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Metadata {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub category_path: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub maintainers: Vec<String>,
    pub http: Option<Http>,
    #[serde(default)]
    pub auth: String,
    #[serde(default)]
    pub description: String,
    pub description_file: Option<String>,
    #[serde(default)]
    pub export_targets: Vec<String>,
    #[serde(default)]
    pub examples: Vec<Example>,
    #[serde(default)]
    pub deprecated: bool,
    pub deprecation_message: Option<String>,
    #[serde(default)]
    pub project: String,
    pub config_json: Option<String>,
    #[serde(default)]
    pub command_id: i32,
    #[serde(default)]
    pub docs: Documentation,
}
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Documentation {
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub response_mode: String,
    #[serde(default)]
    pub response_content_type: String,
    #[serde(default)]
    pub auth: String,
    #[serde(default)]
    pub message_field: String,
    #[serde(default)]
    pub response_schema_json: String,
}
#[derive(Clone, Debug)]
pub struct Endpoint {
    pub source: String,
    pub staged: String,
    pub package: String,
    pub group: String,
    pub controller: String,
    pub action: String,
    pub path: String,
    pub meta: Metadata,
    pub config: Value,
}
impl Endpoint {
    pub fn name(&self) -> String {
        self.source.trim_end_matches(".proto").replace('/', ".")
    }
    pub fn auth(&self) -> String {
        self.meta.auth.to_lowercase()
    }
    pub fn method(&self) -> &str {
        self.meta
            .http
            .as_ref()
            .map(|h| h.method.as_str())
            .filter(|v| !v.is_empty() && *v != "METHOD_UNSPECIFIED")
            .unwrap_or("")
    }
    pub fn manifest(&self) -> Value {
        json!({"NAME":self.name(),"DESC":self.meta.description,"TITLE":self.meta.title,"MAINTAINER":self.meta.maintainers,"TREE":self.meta.category_path.join("-"),"TAGS":self.meta.tags,"CMD":self.meta.command_id,"OUT_EXAMPLE":self.meta.examples.iter().find_map(|e|e.response_json.as_ref()).map(|s|format!("{s}\n")).unwrap_or_default(),"IN_EXAMPLE":self.meta.examples.iter().find_map(|e|e.request_json.as_ref()).map(|s|format!("{s}\n")).unwrap_or_default(),"CONFIG":self.config,"AUTH":self.auth(),"METHOD":self.method(),"PROJECT":self.meta.project})
    }
    pub fn runtime_api(&self) -> crate::Api {
        let mut meta = BTreeMap::new();
        for (k, v) in [
            ("TITLE", self.meta.title.clone()),
            ("AUTH", self.auth()),
            ("TREE", self.meta.category_path.join("-")),
            ("METHOD", self.method().into()),
            ("DESC", self.meta.description.clone()),
            ("TAGS", self.meta.tags.join(",")),
            ("MAINTAINER", self.meta.maintainers.join(",")),
            ("PROJECT", self.meta.project.clone()),
        ] {
            if !v.is_empty() {
                meta.insert(k.into(), v);
            }
        }
        if self.config.is_object() {
            meta.insert("CONFIG".into(), self.config.to_string());
        }
        crate::Api {
            source: self.source.clone(),
            path: self.path.clone(),
            method: if self.method().is_empty() {
                "POST".into()
            } else {
                self.method().into()
            },
            auth: self.auth(),
            title: self.meta.title.clone(),
            input: format!("{}.in", self.package),
            output: format!("{}.out", self.package),
            metadata: meta,
        }
    }
}
pub struct Workspace {
    pub stage: tempfile::TempDir,
    pub files: BTreeMap<String, String>,
    pub endpoints: Vec<Endpoint>,
    pub descriptor: Vec<u8>,
    pub pool: DescriptorPool,
    pub fields: crate::field_model::Fields,
    pub rules: Value,
    pub target: String,
}

pub fn files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = vec![];
    for entry in fs::read_dir(root)? {
        let p = entry?.path();
        if p.file_name().unwrap().to_string_lossy().starts_with('.') {
            continue;
        }
        if p.is_dir() {
            out.extend(files(&p)?)
        } else if p.extension().is_some_and(|v| v == "proto") {
            out.push(p)
        }
    }
    out.sort();
    Ok(out)
}
pub fn protoc() -> PathBuf {
    std::env::var_os("PROTOC")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            if let Ok(executable) = std::env::current_exe() {
                if let Some(directory) = executable.parent() {
                    let sibling = directory.join("protoc");
                    if sibling.is_file() {
                        return sibling;
                    }
                }
            }
            let bundled = Path::new(env!("CARGO_MANIFEST_DIR")).join("toolchain/bin/protoc");
            if bundled.exists() {
                bundled
            } else {
                PathBuf::from("protoc")
            }
        })
}
pub fn command(cmd: &mut Command) -> Result<Vec<u8>> {
    let output = cmd.output().context("cannot start compiler/formatter")?;
    ensure!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(output.stdout)
}
pub fn compile(source: &Path, target: &str) -> Result<Workspace> {
    let source = source.canonicalize()?;
    let stage = tempfile::tempdir()?;
    let mut mapped = BTreeMap::new();
    let mut endpoints = vec![];
    ensure!(
        fs::read_to_string(source.join("common/api_options.proto"))? == SCHEMA,
        "common/api_options.proto differs from the pinned generator schema"
    );
    let field_schema = source.join("common/field_options.proto");
    if field_schema.exists() {
        ensure!(
            fs::read_to_string(field_schema)? == crate::field_model::SCHEMA,
            "common/field_options.proto differs from the pinned generator schema"
        );
    }
    fs::create_dir_all(stage.path().join("common"))?;
    fs::write(stage.path().join("common/api_options.proto"), SCHEMA)?;
    fs::create_dir_all(stage.path().join("google/protobuf"))?;
    fs::write(
        stage.path().join("google/protobuf/descriptor.proto"),
        DESCRIPTOR_SCHEMA,
    )?;
    fs::write(
        stage.path().join("common/field_options.proto"),
        crate::field_model::SCHEMA,
    )?;
    let legacy =
        regex::Regex::new(r"(?m)^\s*//\s*__[A-Z_][A-Z_0-9]*\s*:|^\s*```(?:desc|config|in|out)\b")?;
    let package_declaration = regex::Regex::new(r"(?m)^[ \t]*package[ \t]+[A-Za-z_]")?;
    let removed_policy = regex::Regex::new(r"\b(json_int64|default_json|examples_json)\s*:")?;
    for path in files(&source)? {
        let rel = path
            .strip_prefix(&source)?
            .to_str()
            .context("non-UTF8 path")?
            .replace('\\', "/");
        if ["common/api_options.proto", "common/field_options.proto"].contains(&rel.as_str()) {
            continue;
        }
        let mut text = fs::read_to_string(&path)?;
        ensure!(
            !removed_policy.is_match(&crate::code(&text)),
            "json_int64/default_json are removed; use JSON strings for 64-bit integers and common.field.default"
        );
        let is_api = !rel.starts_with("common/");
        let dest = if is_api && target != "typescript" {
            format!("project/{rel}")
        } else {
            rel.clone()
        };
        if is_api {
            ensure!(
                !legacy.is_match(&text),
                "{rel}: only option (common.api) is supported; migrate legacy metadata first"
            );
            ensure!(
                package_declaration.find(&crate::code(&text)).is_none(),
                "{rel}: API package is generated"
            );
            let stem = rel.trim_end_matches(".proto");
            let (group, name) = stem
                .rsplit_once('/')
                .context("API requires group directory")?;
            let (ctrl, action) = name
                .split_once("__")
                .context("API requires controller__action.proto")?;
            let package = format!("proto.project.{}", stem.replace('/', "."));
            if target == "php" {
                let ns = format!("Proto\\Project\\{}\\{}", crate::studly(group), name)
                    .replace('\\', "\\\\");
                text.push_str(&format!("\noption php_namespace = \"{ns}\";\noption java_package = \"com.vipthink.gen.proto.{group}\";\noption go_package = \"proto/{group}/{name};{group}/{name}\";\n"));
            }
            text.push_str(&format!("\npackage {package};\n"));
            endpoints.push(Endpoint {
                source: rel.clone(),
                staged: dest.clone(),
                package,
                group: group.into(),
                controller: ctrl.into(),
                action: action.into(),
                path: format!("/{group}/{ctrl}/{action}"),
                meta: Metadata::default(),
                config: json!([]),
            });
        }
        fs::create_dir_all(stage.path().join(&dest).parent().unwrap())?;
        fs::write(stage.path().join(&dest), text)?;
        mapped.insert(rel, dest);
    }
    ensure!(!endpoints.is_empty(), "no API protocols");
    // Rewrite business imports after every source name is known.
    for dest in mapped.values() {
        let path = stage.path().join(dest);
        let mut text = fs::read_to_string(&path)?;
        for (raw, to) in &mapped {
            if raw != to {
                text = text.replace(&format!("\"{raw}\""), &format!("\"{to}\""));
            }
        }
        fs::write(path, text)?;
    }
    let descriptor = stage.path().join("input.pb");
    let mut cmd = Command::new(protoc());
    cmd.current_dir(stage.path())
        .arg("-I.")
        .arg("--include_imports")
        .arg("--include_source_info")
        .arg(format!("--descriptor_set_out={}", descriptor.display()))
        .args(mapped.values())
        .arg("common/field_options.proto");
    command(&mut cmd)?;
    let pool = DescriptorPool::decode(fs::read(&descriptor)?.as_slice())?;
    let extension = pool
        .get_extension_by_name("common.api")
        .context("missing common.api descriptor")?;
    let fields = crate::field_model::build(&pool, &source)?;
    let rules = crate::field_model::rules(&pool, &fields);
    let mut paths = BTreeSet::new();
    for api in &mut endpoints {
        let file = pool.get_file_by_name(&api.staged).unwrap();
        let options = file.options();
        ensure!(
            options.has_extension(&extension),
            "{}: missing common.api",
            api.source
        );
        api.meta = serde_json::from_value(match options.get_extension(&extension).as_ref() {
            prost_reflect::Value::Message(m) => serde_json::to_value(m)?,
            _ => bail!("common.api must be a message"),
        })?;
        ensure!(
            !api.meta.title.trim().is_empty(),
            "{}: title required",
            api.source
        );
        ensure!(
            ["PUBLIC", "SESSION", "PRIVATE"].contains(&api.meta.auth.as_str()),
            "{}: auth required",
            api.source
        );
        if ["rust", "go", "typescript"].contains(&target) {
            ensure!(
                api.meta.auth != "PRIVATE",
                "{}: target does not implement PRIVATE auth",
                api.source
            );
        }
        if let Some(http) = &api.meta.http {
            if !http.method.is_empty() {
                ensure!(
                    [
                        "METHOD_UNSPECIFIED",
                        "GET",
                        "POST",
                        "PUT",
                        "PATCH",
                        "DELETE",
                        "HEAD",
                        "OPTIONS"
                    ]
                    .contains(&http.method.as_str()),
                    "invalid HTTP method"
                );
            }
            if let Some(p) = &http.path {
                if !p.is_empty() {
                    ensure!(
                        p.starts_with('/') && !p.contains(['\r', '\n', '?', '#']),
                        "invalid HTTP path"
                    );
                    api.path = p.clone();
                }
            }
        }
        ensure!(
            paths.insert(api.path.clone()),
            "duplicate route {}",
            api.path
        );
        if let Some(desc) = &api.meta.description_file {
            ensure!(
                api.meta.description.is_empty(),
                "description and description_file are mutually exclusive"
            );
            let p = source
                .join(&api.source)
                .parent()
                .unwrap()
                .join(desc)
                .canonicalize()?;
            ensure!(p.starts_with(&source), "description_file outside source");
            api.meta.description = fs::read_to_string(p)?;
        }
        api.config = if let Some(s) = &api.meta.config_json {
            let v: Value = serde_json::from_str(s)?;
            ensure!(v.is_object(), "config_json must be an object");
            ensure!(v.as_object().unwrap().keys().all(|k| k == "return_check"), "config_json only registers runtime return_check; move documentation options to typed metadata");
            ensure!(
                v.get("return_check").is_none_or(Value::is_string),
                "return_check must be a string"
            );
            v
        } else {
            json!([])
        };
        let mut extras = serde_json::Map::new();
        if let Some(h) = &api.meta.http {
            if let Some(v) = &h.request_content_type {
                if !v.is_empty() {
                    extras.insert("in_content_type".into(), json!(v));
                }
            }
            if h.path.as_ref().is_some_and(|p| !p.is_empty()) {
                extras.insert("http_path".into(), json!(api.path));
            }
        }
        for example in &api.meta.examples {
            ensure!(
                !(example.response_json.is_some() && example.data_json.is_some()),
                "response_json and data_json are mutually exclusive"
            );
            for data in [
                &example.request_json,
                &example.response_json,
                &example.data_json,
            ]
            .into_iter()
            .flatten()
            {
                let _: Value = serde_json::from_str(data)?;
            }
        }
        if !extras.is_empty() {
            if !api.config.is_object() {
                api.config = json!({});
            }
            api.config.as_object_mut().unwrap().extend(extras);
        }
        if target == "php" {
            api.config = php_arrays(api.config.take());
        }
    }
    let registry = pool.get_extension_by_name("common.doc_fields").unwrap();
    let mut metadata_files = vec![];
    for (rel, dest) in &mapped {
        let file = pool.get_file_by_name(dest).unwrap();
        if file.options().has_extension(&registry) {
            ensure!(
                file.messages().len() == 0 && file.enums().len() == 0,
                "documentation template files cannot contain business types"
            );
            metadata_files.push(rel.clone());
            continue;
        }
        let path = stage.path().join(dest);
        let text = fs::read_to_string(&path)?;
        fs::write(
            &path,
            crate::strip_options::strip(&text, file.file_descriptor_proto())?,
        )?;
    }
    for name in metadata_files {
        let dest = mapped.remove(&name).unwrap();
        fs::remove_file(stage.path().join(dest))?;
    }
    fs::remove_file(stage.path().join("common/api_options.proto"))?;
    fs::remove_file(stage.path().join("common/field_options.proto"))?;
    let mut cmd = Command::new(protoc());
    cmd.current_dir(stage.path())
        .arg("-I.")
        .arg("--include_imports")
        .arg("--include_source_info")
        .arg(format!("--descriptor_set_out={}", descriptor.display()))
        .args(mapped.values());
    command(&mut cmd)?;
    let descriptor = fs::read(descriptor)?;
    let pool = DescriptorPool::decode(descriptor.as_slice())?;
    for api in &endpoints {
        ensure!(
            pool.get_message_by_name(&format!("{}.in", api.package))
                .is_some()
                && pool
                    .get_message_by_name(&format!("{}.out", api.package))
                    .is_some(),
            "{} requires in and out messages",
            api.source
        );
    }
    if target == "typescript" {
        endpoints.sort_by_key(|a| a.name().to_lowercase().replace('_', "!"));
    }
    Ok(Workspace {
        stage,
        files: mapped,
        endpoints,
        descriptor,
        pool,
        fields,
        rules,
        target: target.into(),
    })
}
pub fn php_arrays(v: Value) -> Value {
    match v {
        Value::Object(m) if m.is_empty() => json!([]),
        Value::Object(m) => Value::Object(m.into_iter().map(|(k, v)| (k, php_arrays(v))).collect()),
        Value::Array(a) => Value::Array(a.into_iter().map(php_arrays).collect()),
        v => v,
    }
}
