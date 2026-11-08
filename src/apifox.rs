//! Apifox export from the committed API manifest and protobuf descriptors.
use anyhow::{bail, ensure, Context, Result};
use regex::RegexBuilder;
#[cfg(test)]
use serde_json::json;
use serde_json::{Map, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

pub use crate::settings::{Project, Settings as Config};

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

pub fn api_path(path: &str, replacements: &Map<String, Value>) -> Result<String> {
    let mut path = path.to_owned();
    for (pattern, replacement) in replacements {
        let body = pattern
            .strip_prefix('/')
            .context("path regex must be /pattern/flags")?;
        let (pattern, flags) = body
            .rsplit_once('/')
            .context("path regex must be /pattern/flags")?;
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

pub use crate::openapi::document;

pub fn project_id(project: &Project, env: &str) -> Option<String> {
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
        value.get("success").is_none_or(|v| v == true),
        "Apifox 未确认导入成功"
    );
    let counters = value["data"]["counters"]
        .as_object()
        .context("Apifox 缺少导入结果 counters")?;
    let mut processed = false;
    for (key, value) in counters {
        let count = value.as_u64().context("Apifox 返回无效计数")?;
        ensure!(
            !key.ends_with("Failed") || count == 0,
            "Apifox 导入存在失败项: {key}={count}"
        );
        if matches!(
            key.as_str(),
            "endpointCreated" | "endpointUpdated" | "endpointIgnored"
        ) && count > 0
        {
            processed = true;
        }
    }
    ensure!(
        counters.get("endpointFailed").and_then(Value::as_u64) == Some(0),
        "Apifox 缺少接口失败计数"
    );
    // A complete zero result is a valid no-op when importing an unchanged document.
    // Missing/incomplete results must still fail instead of being mistaken for a no-op.
    let complete_result = ["endpointCreated", "endpointUpdated", "endpointIgnored"]
        .iter()
        .all(|key| counters.get(*key).and_then(Value::as_u64).is_some());
    ensure!(
        processed || complete_result,
        "Apifox 缺少完整的接口处理结果"
    );
    Ok(())
}
pub async fn sync(
    client: &reqwest::Client,
    project: &Project,
    id: &str,
    data: &Value,
) -> Result<()> {
    let endpoint = format!("https://api.apifox.com/v1/projects/{id}/import-openapi?locale=zh-CN");
    sync_endpoint(client, project, id, data, &endpoint).await
}
async fn sync_endpoint(
    client: &reqwest::Client,
    project: &Project,
    _id: &str,
    data: &Value,
    endpoint: &str,
) -> Result<()> {
    let payload = serde_json::json!({"input":serde_json::to_string(data)?,"options":project.import_options.payload()});
    let response = client
        .post(endpoint)
        .header("X-Apifox-Api-Version", "2024-03-28")
        .bearer_auth(&project.apifox_token)
        .header("User-Agent", "Apifox/1.0.0 (https://www.apifox.cn)")
        .json(&payload)
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
    let config = Config::load(generator.source.parent().unwrap())?;
    let workspace = crate::compiler::compile(&generator.source, &generator.target)?;
    let sources: Vec<String> = options
        .files
        .iter()
        .map(|file| {
            file.strip_prefix(root.join("proto/src"))
                .context("协议不在当前项目内")
                .and_then(|p| p.to_str().context("非 UTF-8 路径"))
                .map(str::to_owned)
        })
        .collect::<Result<_>>()?;
    let jobs = plan(&workspace, &config, &sources, &options.env, None)?;
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

/// Shared preflight for the CLI and Web publishing actions.
pub fn plan<'a>(
    workspace: &crate::compiler::Workspace,
    config: &'a Config,
    sources: &[String],
    env: &str,
    selected: Option<&[usize]>,
) -> Result<Vec<(String, &'a Project, String, Value)>> {
    ensure!(!sources.is_empty(), "至少选择一个协议");
    if let Some(ids) = selected {
        ensure!(
            !ids.is_empty() && ids.iter().all(|i| *i < config.project_list.len()),
            "无效的 Apifox 项目选择"
        );
    }
    ensure!(
        ["release", "test"].contains(&env),
        "invalid Apifox environment"
    );
    let apis = &workspace.endpoints;
    // Preflight every protocol/project before the first remote write.
    let mut jobs = Vec::new();
    for rel in sources {
        let api = apis
            .iter()
            .find(|a| a.source == *rel)
            .with_context(|| format!("未找到 {rel}，请先运行 ./gen_proto.sh"))?;
        let tags = api.meta.tags.clone();
        let explicit_targets = &api.meta.export_targets;
        let targets = if explicit_targets.is_empty() {
            None
        } else {
            Some(explicit_targets)
        };
        if let Some(targets) = targets {
            for t in targets {
                ensure!(
                    config
                        .project_list
                        .iter()
                        .any(|p| p.name == *t || p.apifox_project_name == *t),
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
                            ts.iter()
                                .any(|t| t == &p.name || t == &p.apifox_project_name)
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
        let projects: Vec<_> = projects
            .into_iter()
            .filter(|p| {
                selected.is_none_or(|ids| {
                    config
                        .project_list
                        .iter()
                        .position(|x| std::ptr::eq(x, *p))
                        .is_some_and(|i| ids.contains(&i))
                })
            })
            .collect();
        ensure!(
            !projects.is_empty(),
            "没有选中匹配协议标签的可用 Apifox 项目"
        );
        for project in projects {
            let id = project_id(project, env).with_context(|| {
                format!(
                    "selected project {} has no {} project ID",
                    project.apifox_project_name, env
                )
            })?;
            ensure!(
                id.chars().all(|c| c.is_ascii_digit()) && id.parse::<u64>().is_ok_and(|v| v > 0),
                "Apifox 项目 ID 必须为数字"
            );
            jobs.push((
                rel.to_owned(),
                project,
                id,
                document(api, workspace, project)?,
            ));
        }
    }
    let mut identities = std::collections::BTreeSet::new();
    for (_, project, id, doc) in &jobs {
        for (path, methods) in doc["paths"].as_object().unwrap() {
            for method in methods.as_object().unwrap().keys() {
                ensure!(
                    identities.insert((
                        id.clone(),
                        serde_json::to_string(&project.import_options.payload())?,
                        method.clone(),
                        path.clone()
                    )),
                    "duplicate method + published path in selected target"
                );
            }
        }
    }
    Ok(jobs)
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
    fn unchanged_imports_are_successful_when_apifox_reports_processed_endpoints() {
        for counters in [
            json!({"endpointCreated":1,"endpointFailed":0}),
            json!({"endpointUpdated":1,"endpointFailed":0}),
            json!({"endpointCreated":0,"endpointUpdated":0,"endpointIgnored":1,"endpointFailed":0}),
            json!({"endpointCreated":0,"endpointUpdated":0,"endpointIgnored":0,"endpointFailed":0}),
        ] {
            validate_response(200, &json!({"data":{"counters":counters}}).to_string()).unwrap();
        }
    }

    #[tokio::test]
    async fn import_request_and_response_failures() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/import-data", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            for attempt in 0..3 {
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
                assert!(raw.contains("x-apifox-api-version: 2024-03-28"));
                assert!(raw.contains("application/json"));
                let payload: Value =
                    serde_json::from_str(raw.split_once("\r\n\r\n").unwrap().1).unwrap();
                assert_eq!(
                    payload["options"]["schemaOverwriteBehavior"],
                    "OVERWRITE_EXISTING"
                );
                assert_eq!(payload["options"]["deleteUnmatchedResources"], false);
                assert_eq!(payload["input"], r#"{"openapi":"3.1.0"}"#);
                let body = if attempt == 0 {
                    r#"{"data":{"counters":{"endpointCreated":1,"endpointUpdated":0,"endpointIgnored":0,"endpointFailed":0,"schemaFailed":0}}}"#
                } else if attempt == 1 {
                    r#"{"data":{"counters":{"endpointCreated":0,"endpointUpdated":0,"endpointIgnored":1,"endpointFailed":0,"schemaFailed":0}}}"#
                } else {
                    r#"{"data":{"counters":{"endpointCreated":0,"endpointUpdated":0,"endpointIgnored":0,"endpointFailed":0,"schemaFailed":0}}}"#
                };
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
            }
        });
        for _ in 0..3 {
            sync_endpoint(
                &reqwest::Client::new(),
                &project(),
                "123",
                &json!({"openapi":"3.1.0"}),
                &endpoint,
            )
            .await
            .unwrap();
        }
        server.await.unwrap();
        for body in [
            r#"{"success":true}"#,
            r#"{"data":{"counters":{"endpointCreated":1,"endpointFailed":1}}}"#,
            r#"{"data":{"counters":{"endpointCreated":1,"endpointFailed":0,"schemaFailed":1}}}"#,
            r#"{"data":{"counters":{"endpointFailed":0}}}"#,
            r#"{"data":{"counters":{"endpointCreated":0,"endpointFailed":0}}}"#,
            r#"{"data":{"counters":{"endpointIgnored":1,"endpointFailed":1}}}"#,
            r#"{"data":{"counters":{"endpointIgnored":1,"endpointFailed":0,"schemaFailed":1}}}"#,
            r#"{"data":{"counters":{"endpointIgnored":1}}}"#,
            r#"{"data":{"counters":{"endpointIgnored":-1,"endpointFailed":0}}}"#,
        ] {
            assert!(validate_response(200, body).is_err());
        }
        assert!(validate_response(401, r#"{"success":false}"#).is_err());
        assert!(validate_response(200, r#"{"success":false}"#).is_err());
        assert!(validate_response(200, "<html>error</html>").is_err());
        assert!(validate_response(500, r#"{"success":true}"#).is_err());
    }
}
