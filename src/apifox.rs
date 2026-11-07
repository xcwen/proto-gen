//! Apifox export from the committed API manifest and protobuf descriptors.
use crate::Api;
use anyhow::{bail, ensure, Context, Result};
use prost_reflect::{DescriptorPool, FieldDescriptor, Kind, MessageDescriptor};
use regex::RegexBuilder;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Deserialize)]
pub struct Config {
    pub project_list: Vec<Project>,
}
#[derive(Deserialize)]
pub struct Project {
    pub apifox_project_name: String,
    #[serde(default)]
    pub apifox_token: String,
    pub apifox_project_id: Option<Value>,
    pub test_apifox_project_id: Option<Value>,
    #[serde(default)]
    pub url_replace_cfg: Map<String, Value>,
    #[serde(default)]
    pub tag_list: Vec<String>,
    pub export_tags_flag: Option<bool>,
    #[serde(default)]
    pub name: String,
}

const HELP: &str = "用法: sync_apifox.sh [--env release|test] [--dry-run] <协议.proto> [...]\n\n默认 release；--dry-run 或 APIFOX_DRY_RUN=1 只预览，不发送请求。\n支持绝对路径、调用目录/仓库根目录相对路径和相对 proto/ 的路径。\n配置读取 proto/config.json（与 Node 版格式一致）。\n同步前先运行 ./gen_proto.sh；本工具不生成协议或重启服务。";

pub struct Options {
    pub env: String,
    pub dry_run: bool,
    pub files: Vec<PathBuf>,
}
impl Options {
    pub fn parse(args: Vec<String>, root: &Path, caller: &Path) -> Result<Option<Self>> {
        let mut env = "release".to_owned();
        let mut dry_run = std::env::var("APIFOX_DRY_RUN").as_deref() == Ok("1");
        let mut inputs = Vec::new();
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "-h" | "--help" => {
                    println!("{HELP}");
                    return Ok(None);
                }
                "--env" => env = args.next().context("--env 缺少参数")?,
                "release" | "test" => env = arg,
                "--dry-run" => dry_run = true,
                "--" => {
                    inputs.extend(args);
                    break;
                }
                _ if arg.starts_with("--env=") => env = arg[6..].to_owned(),
                _ if arg.starts_with('-') => bail!("未知选项: {arg}"),
                _ => inputs.push(arg),
            }
        }
        ensure!(
            ["release", "test"].contains(&env.as_str()),
            "无效环境: {env}"
        );
        ensure!(!inputs.is_empty(), "请至少指定一个协议文件\n{HELP}");
        let source = root.join("proto/src").canonicalize()?;
        let mut files = Vec::new();
        for input in inputs {
            ensure!(
                input.ends_with(".proto"),
                "协议文件必须以 .proto 结尾: {input}"
            );
            let path = [
                caller.join(&input),
                root.join(&input),
                root.join("proto").join(&input),
            ]
            .into_iter()
            .find(|p| p.is_file())
            .with_context(|| format!("协议文件不存在: {input}"))?
            .canonicalize()?;
            ensure!(
                path.starts_with(&source),
                "协议文件必须位于本项目 proto/src/ 内: {input}"
            );
            fs::File::open(&path).with_context(|| format!("协议文件不可读: {input}"))?;
            if !files.contains(&path) {
                files.push(path);
            }
        }
        Ok(Some(Self {
            env,
            dry_run,
            files,
        }))
    }
}

fn comments(field: &FieldDescriptor) -> String {
    let file = field.parent_file();
    file.file_descriptor_proto()
        .source_code_info
        .as_ref()
        .and_then(|info| info.location.iter().find(|l| l.path == field.path()))
        .map(|l| {
            format!(
                "{}{}",
                l.leading_comments.as_deref().unwrap_or(""),
                l.trailing_comments.as_deref().unwrap_or("")
            )
        })
        .unwrap_or_default()
        .trim()
        .to_owned()
}
fn field_config(field: &FieldDescriptor) -> Result<(String, Value)> {
    let text = comments(field);
    let re = regex::Regex::new(r"CFG=(.*\})")?;
    if let Some(c) = re.captures(&text) {
        let cfg = serde_json::from_str(&c[1])
            .with_context(|| format!("{} CFG 不是有效 JSON", field.full_name()))?;
        return Ok((re.replace(&text, "").trim().to_owned(), cfg));
    }
    Ok((text, json!({})))
}
fn message_schema(
    message: MessageDescriptor,
    pool: &DescriptorPool,
    stack: &mut BTreeSet<String>,
) -> Result<Value> {
    ensure!(
        stack.insert(message.full_name().to_owned()),
        "结构存在循环引用: {}",
        message.full_name()
    );
    let mut properties = Map::new();
    let mut required = Vec::new();
    for field in message.fields() {
        let (mut description, cfg) = field_config(&field)?;
        if let Some(from) = cfg["from"].as_str() {
            let common = pool
                .all_messages()
                .find(|m| m.name() == "common_field_t")
                .and_then(|m| m.get_field_by_name(from))
                .with_context(|| format!("未找到 common_field_t.{from}"))?;
            description = field_config(&common)?.0;
        }
        let mut schema = match field.kind() {
            Kind::Message(m) if field.is_map() => {
                let value = m.get_field_by_name("value").context("map missing value")?;
                json!({"type":"object", "additionalProperties": kind_schema(value.kind(), pool, stack)?})
            }
            kind => kind_schema(kind, pool, stack)?,
        };
        schema["description"] = json!(description);
        if let Some(t) = cfg.get("field_type") {
            schema["type"] = t.clone();
            if t == "file" {
                schema["type"] = json!("string");
                schema["format"] = json!("binary");
            }
        }
        if let Some(default) = cfg.get("default") {
            schema["default"] = default.clone();
        } else {
            required.push(field.name().to_owned());
        }
        if let Some(example) = cfg.get("example") {
            schema["examples"] = json!([example]);
        }
        if field.is_list() {
            schema = json!({"type": if cfg.get("default").is_some() { json!(["array", null]) } else { json!(["array"]) }, "description": description, "items":schema});
        }
        properties.insert(field.name().to_owned(), schema);
    }
    stack.remove(message.full_name());
    Ok(json!({"type":"object", "properties":properties,"required":required}))
}
fn kind_schema(kind: Kind, pool: &DescriptorPool, stack: &mut BTreeSet<String>) -> Result<Value> {
    Ok(match kind {
        Kind::Message(m) => message_schema(m, pool, stack)?,
        Kind::Bool => json!({"type":"boolean"}),
        Kind::String => json!({"type":"string"}),
        Kind::Bytes => json!({"type":"string", "format":"byte"}),
        Kind::Float | Kind::Double => json!({"type":"number"}),
        _ => json!({"type":"integer"}),
    })
}
fn input_schema(api: &Api, pool: &DescriptorPool) -> Result<Value> {
    let name = format!("{}.apidoc_in", api.input.trim_end_matches(".in"));
    if let Some(message) = pool.get_message_by_name(&name) {
        let groups: Vec<_> = message.oneofs().filter(|o| !o.is_synthetic()).collect();
        ensure!(
            groups.len() == 1 && message.fields().all(|f| f.containing_oneof().is_some()),
            "{name} 只能包含一个 oneof"
        );
        let group = &groups[0];
        let mut branches = Vec::new();
        for field in group.fields() {
            let Kind::Message(m) = field.kind() else {
                bail!("{name} oneof 必须使用 message 类型");
            };
            let mut schema = message_schema(m, pool, &mut BTreeSet::new())?;
            ensure!(
                schema["properties"].get(group.name()).is_none(),
                "oneof 与字段同名: {}",
                group.name()
            );
            schema["properties"][group.name()] = json!({"type":"string", "const":field.name(), "description":field_config(&field)?.0});
            schema["required"]
                .as_array_mut()
                .unwrap()
                .insert(0, json!(group.name()));
            schema["x-apifox-orders"] = json!(std::iter::once(group.name().to_owned())
                .chain(
                    schema["properties"]
                        .as_object()
                        .unwrap()
                        .keys()
                        .filter(|k| *k != group.name())
                        .cloned()
                )
                .collect::<Vec<_>>());
            schema["x-apifox-ignore-properties"] = json!([]);
            branches.push(schema);
        }
        ensure!(!branches.is_empty(), "{name} 未定义有效的 oneof 字段");
        return Ok(json!({"oneOf":branches}));
    }
    message_schema(
        pool.get_message_by_name(&api.input)
            .context("缺少输入描述，请先运行 ./gen_proto.sh")?,
        pool,
        &mut BTreeSet::new(),
    )
}

pub fn api_path(path: &str, replacements: &Map<String, Value>) -> Result<String> {
    let mut path = path.to_owned();
    for (pattern, replacement) in replacements {
        let Some(body) = pattern.strip_prefix('/') else {
            continue;
        };
        let Some((pattern, flags)) = body.rsplit_once('/') else {
            continue;
        };
        ensure!(
            flags.chars().all(|f| "gimsu".contains(f)),
            "不支持的路径正则标志: {flags}"
        );
        let regex = RegexBuilder::new(pattern)
            .case_insensitive(flags.contains('i'))
            .multi_line(flags.contains('m'))
            .dot_matches_new_line(flags.contains('s'))
            .build()
            .context("不支持的路径替换正则")?;
        let replacement = replacement.as_str().context("路径替换值必须是字符串")?;
        // JavaScript $& is Rust's ${0}; numbered capture groups work in both.
        let replacement = replacement.replace("$&", "${0}");
        path = if flags.contains('g') {
            regex.replace_all(&path, replacement.as_str())
        } else {
            regex.replace(&path, replacement.as_str())
        }
        .into_owned();
    }
    Ok(path)
}

pub fn document(
    api: &Api,
    pool: &DescriptorPool,
    source: &str,
    project: &Project,
) -> Result<Value> {
    let cfg: Value = api
        .metadata
        .get("CONFIG")
        .map(|s| serde_json::from_str(s))
        .transpose()?
        .unwrap_or_else(|| json!({}));
    let _ = source;
    let input = input_schema(api, pool)?;
    let mut output = message_schema(
        pool.get_message_by_name(&api.output)
            .context("缺少输出描述，请先运行 ./gen_proto.sh")?,
        pool,
        &mut BTreeSet::new(),
    )?;
    output["description"] = json!("数据区");
    let folder = api
        .metadata
        .get("PROJECT")
        .filter(|s| !s.is_empty())
        .map(String::as_str)
        .unwrap_or("开发中");
    let description = api.metadata.get("DESC").cloned().unwrap_or_default();
    let mut operation = json!({
        "summary":api.title, "description":description,
        "x-apifox-folder":folder, "x-apifox-status":cfg.get("status").unwrap_or(&json!("released")),
        "parameters":cfg.get("parameters").unwrap_or(&json!([])),
        "requestBody":{"content":{cfg["in_content_type"].as_str().unwrap_or("application/json"):{"schema":input,"examples":cfg.get("examples").unwrap_or(&json!({}))}}},
        "responses":{"200":{"description":"成功","content":{"application/json":{"schema":{
            "type":"object","title":"ResponseModel","properties":{
                "code":{"type":"integer","description":"错误码( 0:成功. 其它值:失败[无data数据区])"},
                "info":{"type":"string","description":"错误信息","examples":["succ"]},
                "request_id":{"type":"string","description":"请求id,用于定位请求","examples":["21al1389334aal4f"]},
                "data":output},"required":["code","info","request_id"]}}}}}
    });
    if project.export_tags_flag != Some(false) {
        let mut tags = tags(api);
        if folder != "开发中" {
            tags.push(format!("PROJECT:{folder}"));
        }
        tags.push("rust".into());
        let mut seen = BTreeSet::new();
        tags.retain(|t| seen.insert(t.clone()));
        operation["tags"] = json!(tags);
    }
    let path = api_path(&api.path, &project.url_replace_cfg)?;
    // Use the generated HTTP method so the document matches Rust's registered routes.
    Ok(
        json!({"openapi":if input.get("oneOf").is_some() {"3.1.0"} else {"3.0.1"},
        "paths":{path:{api.method.to_lowercase():operation}}}),
    )
}
fn tags(api: &Api) -> Vec<String> {
    api.metadata
        .get("TAGS")
        .into_iter()
        .flat_map(|s| s.split(','))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}
fn project_id(project: &Project, env: &str) -> Option<String> {
    let value = if env == "release" {
        &project.apifox_project_id
    } else {
        &project.test_apifox_project_id
    };
    value.as_ref().and_then(|v| match v {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) if n.as_u64().is_some_and(|n| n > 0) => Some(n.to_string()),
        _ => None,
    })
}

/// Only accept an explicit successful Apifox response. Never print response bodies or credentials.
pub fn validate_response(status: u16, body: &str) -> Result<()> {
    ensure!((200..300).contains(&status), "Apifox 返回 HTTP {status}");
    let value: Value = serde_json::from_str(body).context("Apifox 返回非 JSON 响应")?;
    ensure!(
        value["success"] == true,
        "Apifox 未确认导入成功，请检查项目权限和配置"
    );
    Ok(())
}
async fn sync(client: &reqwest::Client, project: &Project, id: &str, data: &Value) -> Result<()> {
    let endpoint = format!("https://api.apifox.cn/api/v1/projects/{id}/import-data?locale=zh-CN");
    sync_endpoint(client, project, id, data, &endpoint).await
}
async fn sync_endpoint(
    client: &reqwest::Client,
    project: &Project,
    id: &str,
    data: &Value,
    endpoint: &str,
) -> Result<()> {
    let form = [
        ("importFormat", "openapi"),
        ("importBasePath", "false"),
        ("importFullPath", "false"),
        ("ignoreEnvironment", "false"),
        ("apiFolderId", "0"),
        ("schemaFolderId", "0"),
        ("testSuiteFolderId", "0"),
        ("testCaseFolderId", "0"),
        ("apiOverwriteMode", "name"),
        ("schemaOverwriteMode", "ignore"),
        ("docOverwriteMode", "ignore"),
        ("testSuiteOverwriteMode", "ignore"),
        ("testCaseOverwriteMode", "ignore"),
        ("data", &serde_json::to_string(data)?),
        ("autoImport", "true"),
        ("autoImportId", ""),
        ("url", ""),
        ("auth", ""),
        ("syncApiFolder", ""),
    ];
    let response = client
        .post(endpoint)
        .header("X-Project-Id", id)
        .header("X-Apifox-Version", "2022-11-16")
        .bearer_auth(&project.apifox_token)
        .header("User-Agent", "Apifox/1.0.0 (https://www.apifox.cn)")
        .form(&form)
        .send()
        .await
        .context("Apifox 请求失败")?;
    let status = response.status().as_u16();
    validate_response(
        status,
        &response.text().await.context("读取 Apifox 响应失败")?,
    )
}

pub async fn run(args: Vec<String>) -> Result<()> {
    let config_path = std::env::var_os("PROTO_GEN_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("proto/generator.toml"));
    run_config(args, &config_path).await
}
pub async fn run_config(args: Vec<String>, config_path: &Path) -> Result<()> {
    let generator = crate::compiler::Config::load(config_path)?;
    let root = generator.root().canonicalize()?;
    let Some(options) = Options::parse(args, &root, &std::env::current_dir()?)? else {
        return Ok(());
    };
    let config: Config = if generator.php() {
        let value: Value = serde_json::from_slice(&crate::compiler::command(
            std::process::Command::new("php")
                .arg("-r")
                .arg("echo json_encode(require $argv[1], JSON_THROW_ON_ERROR);")
                .arg(root.join("proto/config.php")),
        )?)?;
        Config {
            project_list: value["export"]
                .as_object()
                .map(|m| {
                    m.iter()
                        .map(|(name, v)| {
                            let mut v = v.clone();
                            v["name"] = json!(name);
                            serde_json::from_value(v)
                        })
                        .collect::<std::result::Result<Vec<_>, _>>()
                })
                .transpose()?
                .unwrap_or_default(),
        }
    } else {
        serde_json::from_slice(&fs::read(root.join("proto/config.json"))?)?
    };
    let workspace = crate::compiler::compile(&generator.source, &generator.target)?;
    let apis: Vec<_> = workspace
        .endpoints
        .iter()
        .map(|a| a.runtime_api())
        .collect();
    let pool = workspace.pool;
    // Preflight every protocol/project before the first remote write.
    let mut jobs = Vec::new();
    for file in &options.files {
        let rel = file
            .strip_prefix(root.join("proto/src"))?
            .to_str()
            .context("非 UTF-8 路径")?;
        let api = apis
            .iter()
            .find(|a| a.source == rel)
            .with_context(|| format!("未找到 {rel}，请先运行 ./gen_proto.sh"))?;
        let tags = tags(api);
        let settings: Value = api
            .metadata
            .get("CONFIG")
            .map(|s| serde_json::from_str(s))
            .transpose()?
            .unwrap_or_else(|| json!({}));
        let targets = settings.get("export_targets").and_then(Value::as_array);
        if let Some(targets) = targets {
            for t in targets {
                ensure!(
                    t.as_str().is_some_and(|n| config
                        .project_list
                        .iter()
                        .any(|p| p.name == n || p.apifox_project_name == n)),
                    "未配置的导出目标"
                );
            }
        }
        let projects: Vec<_> = config
            .project_list
            .iter()
            .filter(|p| {
                !p.apifox_token.is_empty()
                    && targets
                        .map(|ts| {
                            ts.iter().any(|t| {
                                t.as_str() == Some(&p.name)
                                    || t.as_str() == Some(&p.apifox_project_name)
                            })
                        })
                        .unwrap_or_else(|| p.tag_list.iter().any(|t| tags.contains(t)))
            })
            .collect();
        ensure!(
            !projects.is_empty(),
            "{rel} 没有设置有效的标签；协议标签: {}；有效标签: {}",
            tags.join(","),
            config
                .project_list
                .iter()
                .filter(|p| !p.apifox_token.is_empty())
                .flat_map(|p| p.tag_list.clone())
                .collect::<Vec<_>>()
                .join(",")
        );
        for project in projects {
            let Some(id) = project_id(project, &options.env) else {
                println!(
                    "跳过: {} 缺少 {}",
                    project.apifox_project_name,
                    if options.env == "release" {
                        "apifox_project_id"
                    } else {
                        "test_apifox_project_id"
                    }
                );
                continue;
            };
            ensure!(
                id.chars().all(|c| c.is_ascii_digit()),
                "Apifox 项目 ID 必须为数字"
            );
            jobs.push((
                rel.to_owned(),
                project,
                id,
                document(api, &pool, &fs::read_to_string(file)?, project)?,
            ));
        }
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    for (rel, project, id, data) in &jobs {
        println!(
            "同步到:[{}]{}:{id}: {rel}",
            options.env, project.apifox_project_name
        );
        if options.dry_run {
            println!("{}", serde_json::to_string_pretty(data)?);
        } else {
            sync(&client, project, id, data)
                .await
                .with_context(|| format!("同步失败，停止后续协议: {rel}"))?;
            println!("导入成功");
        }
    }
    println!(
        "{}，共 {} 个协议、{} 次导入{}。",
        if options.dry_run {
            "预览完成"
        } else {
            "同步完成"
        },
        options.files.len(),
        jobs.len(),
        if options.dry_run {
            "，未发送网络请求"
        } else {
            ""
        }
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> Project {
        serde_json::from_value(json!({
            "apifox_project_name":"test", "apifox_token":"test-token",
            "apifox_project_id":"123", "test_apifox_project_id":456,
            "url_replace_cfg":{"/^/demo//":"/v1/"}, "tag_list":["demo"]
        }))
        .unwrap()
    }

    #[test]
    fn node_config_and_ordered_path_replacements() {
        let p = project();
        assert_eq!(project_id(&p, "release").as_deref(), Some("123"));
        assert_eq!(project_id(&p, "test").as_deref(), Some("456"));
        assert_eq!(
            api_path("/demo/a/get", &p.url_replace_cfg).unwrap(),
            "/v1/a/get"
        );
        let replacements: Map<String, Value> =
            serde_json::from_str(r#"{"/z/g":"a","/a/g":"b"}"#).unwrap();
        assert_eq!(api_path("z", &replacements).unwrap(), "b");
        let replacements = serde_json::from_value(json!({"/(a)/g":"$1$&"})).unwrap();
        assert_eq!(api_path("aa", &replacements).unwrap(), "aaaa");
        assert!(api_path("a", &serde_json::from_value(json!({"/a/y":"b"})).unwrap()).is_err());
    }

    #[test]
    fn paths_preflight_deduplicates_and_rejects_escape() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("proto/src/demo")).unwrap();
        fs::write(root.join("proto/src/demo/a__get.proto"), "").unwrap();
        let args = vec![
            "--env=test",
            "proto/src/demo/a__get.proto",
            "src/demo/a__get.proto",
        ];
        let opts = Options::parse(args.into_iter().map(str::to_owned).collect(), root, root)
            .unwrap()
            .unwrap();
        assert_eq!(opts.files.len(), 1);
        assert_eq!(opts.env, "test");
        for args in [
            vec!["--env=other", "proto/src/demo/a__get.proto"],
            vec!["--unknown"],
            vec!["missing.proto"],
            vec!["file.txt"],
        ] {
            assert!(
                Options::parse(args.into_iter().map(str::to_owned).collect(), root, root).is_err()
            );
        }
        fs::write(root.join("outside.proto"), "").unwrap();
        assert!(Options::parse(vec!["outside.proto".into()], root, root).is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(
                root.join("outside.proto"),
                root.join("proto/src/demo/escape.proto"),
            )
            .unwrap();
            assert!(
                Options::parse(vec!["proto/src/demo/escape.proto".into()], root, root).is_err()
            );
        }
    }

    #[test]
    fn exports_descriptors_cfg_common_fields_oneof_and_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("proto/src");
        fs::create_dir_all(source.join("demo")).unwrap();
        fs::create_dir_all(source.join("common")).unwrap();
        fs::write(
            source.join("common/common.proto"),
            r#"syntax="proto3";
package common;
message common_field_t {
  string title=1; // 公共标题 CFG={"example":"shared"}
}
message Item {
  string title=1; // 本地描述 CFG={"from":"title", "default":"", "example":"local"}
}
"#,
        )
        .unwrap();
        let text = r#"syntax="proto3";
import "common/common.proto";
import "common/api_options.proto";
option (common.api) = {
 title:"测试接口" auth:SESSION tags:"demo" tags:"other" project:"测试项目"
 description:"多行描述\n"
 config_json:'{"status":"developing","examples":{"one":{"value":{}}}}'
};
message in {}
message Branch {
  repeated common.Item items=1; // 数据 CFG={"default":[]}
  bytes image=2; // 上传 CFG={"field_type":"file"}
}
message apidoc_in {
  oneof kind { Branch create=1; }
}
message out {
  common.Item item=1;
}
"#;
        fs::write(
            source.join("common/api_options.proto"),
            crate::compiler::SCHEMA,
        )
        .unwrap();
        fs::write(source.join("demo/a__get.proto"), text).unwrap();
        let output = temp.path().join("generated");
        crate::generate(&source, &output).unwrap();
        let apis: Vec<Api> =
            serde_json::from_slice(&fs::read(output.join("api.json")).unwrap()).unwrap();
        let pool =
            DescriptorPool::decode(fs::read(output.join("descriptor.bin")).unwrap().as_slice())
                .unwrap();
        let mut p = project();
        let doc = document(&apis[0], &pool, text, &p).unwrap();
        assert_eq!(doc["openapi"], "3.1.0");
        let op = &doc["paths"]["/v1/a/get"]["post"];
        assert_eq!(op["x-apifox-folder"], "测试项目");
        assert_eq!(op["x-apifox-status"], "developing");
        assert!(op["description"].as_str().unwrap().contains("多行描述"));
        assert_eq!(
            op["tags"],
            json!(["demo", "other", "PROJECT:测试项目", "rust"])
        );
        let branch = &op["requestBody"]["content"]["application/json"]["schema"]["oneOf"][0];
        assert_eq!(branch["properties"]["kind"]["const"], "create");
        assert_eq!(branch["required"], json!(["kind", "image"]));
        assert_eq!(
            branch["properties"]["items"]["type"],
            json!(["array", null])
        );
        let title = &branch["properties"]["items"]["items"]["properties"]["title"];
        assert_eq!(title["description"], "公共标题");
        assert_eq!(title["examples"], json!(["local"]));
        assert_eq!(branch["properties"]["image"]["format"], "binary");
        assert_eq!(
            op["responses"]["200"]["content"]["application/json"]["schema"]["required"],
            json!(["code", "info", "request_id"])
        );
        p.export_tags_flag = Some(false);
        assert!(
            document(&apis[0], &pool, text, &p).unwrap()["paths"]["/v1/a/get"]["post"]
                .get("tags")
                .is_none()
        );
    }

    #[tokio::test]
    async fn import_request_and_response_failures() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/import-data", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            loop {
                let mut buf = [0; 4096];
                let n = socket.read(&mut buf).await.unwrap();
                assert!(n > 0);
                request.extend_from_slice(&buf[..n]);
                let raw = String::from_utf8_lossy(&request);
                if let Some((headers, body)) = raw.split_once("\r\n\r\n") {
                    let length: usize = headers
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length: ")
                                .map(str::to_owned)
                        })
                        .unwrap()
                        .parse()
                        .unwrap();
                    if body.len() >= length {
                        break;
                    }
                }
            }
            let raw = String::from_utf8(request).unwrap();
            assert!(raw.starts_with("POST /import-data HTTP/1.1"));
            assert!(raw.contains("authorization: Bearer test-token"));
            assert!(raw.contains("x-project-id: 123"));
            assert!(raw.contains("application/x-www-form-urlencoded"));
            assert!(raw.contains("apiOverwriteMode=name"));
            assert!(raw.contains("data=%7B%22openapi%22%3A%223.0.1%22%7D"));
            let body = r#"{"success":true,"data":{}}"#;
            socket
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        });
        sync_endpoint(
            &reqwest::Client::new(),
            &project(),
            "123",
            &json!({"openapi":"3.0.1"}),
            &endpoint,
        )
        .await
        .unwrap();
        server.await.unwrap();
        assert!(validate_response(401, r#"{"success":false}"#).is_err());
        assert!(validate_response(200, r#"{"success":false}"#).is_err());
        assert!(validate_response(200, "<html>error</html>").is_err());
        assert!(validate_response(500, r#"{"success":true}"#).is_err());
    }
}
