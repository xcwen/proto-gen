use anyhow::{bail, Context, Result};
use std::path::PathBuf;
fn main() {
    if let Err(e) = run() {
        eprintln!("proto-gen: {e:#}");
        std::process::exit(1);
    }
}
fn run() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let cmd = args.next().unwrap_or_else(|| "help".into());
    if cmd == "--version" {
        println!("proto-gen {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if cmd == "help" || cmd == "--help" {
        println!("proto-gen generate --config proto/generator.toml [--check] [--force]\nproto-gen openapi --config proto/generator.toml --out openapi.json\nproto-gen sync-apifox --config proto/generator.toml [--dry-run] <protocol.proto>\nproto-gen serve [--config proto/generator.toml ...] [--listen 127.0.0.1:7337] [--token-env NAME]\nproto-gen --version");
        return Ok(());
    }
    if cmd == "serve" || cmd == "web" {
        let mut configs = vec![];
        let mut listen = "127.0.0.1:7337".parse()?;
        let mut token_env = None;
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--config" => {
                    configs.push(PathBuf::from(args.next().context("missing config path")?))
                }
                "--listen" => listen = args.next().context("missing listen address")?.parse()?,
                "--token-env" => {
                    token_env = Some(
                        args.next()
                            .context("missing token environment variable name")?,
                    )
                }
                "--help" | "-h" => {
                    println!("proto-gen serve [--config proto/generator.toml ...] [--listen 127.0.0.1:7337] [--token-env NAME]");
                    return Ok(());
                }
                _ => bail!("unknown Web option: {arg}"),
            }
        }
        if configs.is_empty() {
            configs.push(PathBuf::from("proto/generator.toml"));
        }
        return tokio::runtime::Runtime::new()?
            .block_on(workspace_proto_gen::web::serve(configs, listen, token_env));
    }
    let mut config = None;
    let mut check = false;
    let mut force = false;
    let mut out = None;
    let mut rest = vec![];
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" => config = Some(PathBuf::from(args.next().context("missing config path")?)),
            "--check" if cmd == "generate" => check = true,
            "--force" if cmd == "generate" => force = true,
            "--out" if cmd == "openapi" => {
                out = Some(PathBuf::from(args.next().context("missing output path")?))
            }
            _ => rest.push(arg),
        }
    }
    let config = config.unwrap_or_else(|| PathBuf::from("proto/generator.toml"));
    match cmd.as_str() {
        "generate" => {
            anyhow::ensure!(rest.is_empty(), "unknown options: {rest:?}");
            workspace_proto_gen::project::run_with_options(&config, check, force)
        }
        "openapi" => {
            anyhow::ensure!(rest.is_empty(), "unknown options: {rest:?}");
            let c = workspace_proto_gen::compiler::Config::load(&config)?;
            let w = workspace_proto_gen::compiler::compile(&c.source, &c.target)?;
            let project = serde_json::from_value(
                serde_json::json!({"apifox_project_name":"local","url_replace_cfg":{},"tag_list":[]}),
            )?;
            let mut paths = serde_json::Map::new();
            let mut schemas = serde_json::Map::new();
            for a in &w.endpoints {
                let doc = workspace_proto_gen::apifox::document(a, &w, &project)?;
                for (path, methods) in doc["paths"].as_object().unwrap() {
                    let target = paths
                        .entry(path.clone())
                        .or_insert_with(|| serde_json::json!({}));
                    for (method, operation) in methods.as_object().unwrap() {
                        anyhow::ensure!(
                            target.get(method).is_none(),
                            "duplicate published method and path"
                        );
                        target[method] = operation.clone();
                    }
                }
                schemas.extend(doc["components"]["schemas"].as_object().unwrap().clone());
            }
            let doc = serde_json::json!({"openapi":"3.1.0","info":{"title":"Generated API","version":"1"},"paths":paths,"components":{"schemas":schemas}});
            workspace_proto_gen::openapi::validate_document(&doc)?;
            workspace_proto_gen::project::put(
                &out.context("--out is required")?,
                serde_json::to_vec_pretty(&doc)?,
            )
        }
        "sync-apifox" => tokio::runtime::Runtime::new()?
            .block_on(workspace_proto_gen::apifox::run_config(rest, &config)),
        _ => bail!("unknown command {cmd}"),
    }
}
