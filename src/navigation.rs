//! Editor navigation contract. Paths are project-relative, never build-stage paths.
use crate::compiler::{Config, Workspace};
use anyhow::{Context, Result};
use heck::{ToSnakeCase, ToUpperCamelCase};
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Serialize)]
pub struct Index {
    pub version: u32,
    pub endpoints: Vec<Endpoint>,
}
#[derive(Debug, Serialize)]
pub struct Endpoint {
    pub id: String,
    pub method: String,
    pub paths: Vec<String>,
    pub proto: String,
    pub controller: Controller,
}
#[derive(Debug, Serialize)]
pub struct Controller {
    pub file: String,
    pub owner: String,
    pub symbol: String,
}
fn relative(root: &Path, path: &Path) -> Result<String> {
    Ok(crate::project::clean(path)
        .strip_prefix(crate::project::clean(root))
        .context("navigation target must be inside project")?
        .to_string_lossy()
        .replace('\\', "/"))
}
pub fn index(cfg: &Config, work: &Workspace) -> Result<Index> {
    let root = cfg.root();
    let mut endpoints = Vec::new();
    for a in &work.endpoints {
        let (group, file, owner, symbol) = match cfg.target.as_str() {
            "php" => {
                let class = crate::studly(&a.controller);
                (
                    crate::studly(&a.group),
                    format!("{class}.php"),
                    class,
                    a.action.clone(),
                )
            }
            "typescript" => (
                a.group.clone(),
                format!("{}.ts", a.controller),
                a.controller.clone(),
                a.action.clone(),
            ),
            "go" => (
                a.group.clone(),
                format!("{}.go", a.controller),
                format!("{}Controller", crate::studly(&a.controller)),
                crate::studly(&a.action),
            ),
            "rust" => {
                let controller = a.controller.to_snake_case();
                (
                    a.group
                        .split('/')
                        .map(ToSnakeCase::to_snake_case)
                        .collect::<Vec<_>>()
                        .join("/"),
                    format!("{controller}.rs"),
                    controller.to_upper_camel_case(),
                    crate::controllers::identifier(&a.action.to_snake_case())?,
                )
            }
            _ => anyhow::bail!("unknown navigation target"),
        };
        let mut paths = vec![a.path.clone()];
        if cfg.php() && a.meta.tags.iter().any(|t| t == "_uri_no_project") {
            let alias = format!("/{}/{}", a.controller, a.action);
            if !paths.contains(&alias) {
                paths.push(alias);
            }
        }
        endpoints.push(Endpoint {
            id: format!("{}/{}/{}", a.group, a.controller, a.action),
            method: a.runtime_api().method,
            paths,
            proto: relative(&root, &cfg.source.join(&a.source))?,
            controller: Controller {
                file: relative(&root, &cfg.controllers.join(group).join(file))?,
                owner,
                symbol,
            },
        });
    }
    endpoints.sort_by(|a, b| (&a.id, &a.method).cmp(&(&b.id, &b.method)));
    Ok(Index {
        version: 1,
        endpoints,
    })
}
