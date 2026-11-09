pub mod apifox;
mod controllers;
pub use controllers::generate_controllers;

use anyhow::{bail, Result};
use heck::ToSnakeCase;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Api {
    pub source: String,
    pub path: String,
    pub method: String,
    pub auth: String,
    pub title: String,
    pub input: String,
    pub output: String,
    pub metadata: BTreeMap<String, String>,
}
fn files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = vec![];
    for entry in fs::read_dir(root)? {
        let p = entry?.path();
        if p.file_name().unwrap().to_string_lossy().starts_with('.') {
            continue;
        }
        if p.is_dir() {
            out.extend(files(&p)?);
        } else if p.extension().is_some_and(|e| e == "proto") {
            out.push(p);
        }
    }
    out.sort();
    Ok(out)
}
fn write_changed(path: &Path, data: &[u8]) -> Result<()> {
    if fs::read(path).ok().as_deref() != Some(data) {
        fs::create_dir_all(path.parent().unwrap())?;
        fs::write(path, data)?;
    }
    Ok(())
}
// Strip comments while preserving quoted strings and line boundaries.
fn code(text: &str) -> String {
    Regex::new(r#"(?s)/\*.*?\*/|//[^\n]*|"(?:\\.|[^"\\])*""#)
        .unwrap()
        .replace_all(text, |c: &regex::Captures<'_>| {
            if c[0].starts_with('"') {
                c[0].to_string()
            } else {
                c[0].chars()
                    .map(|v| if v == '\n' { '\n' } else { ' ' })
                    .collect()
            }
        })
        .into_owned()
}
pub mod compiler;
pub mod controller_edit;
pub mod field_model;
pub mod fields;
mod incremental;
pub mod openapi;
pub mod project;
pub mod settings;
pub mod strip_options;
pub mod targets;
pub fn studly(value: &str) -> String {
    value
        .split('_')
        .map(|s| {
            let mut c = s.chars();
            c.next()
                .map(|x| x.to_uppercase().collect::<String>() + c.as_str())
                .unwrap_or_default()
        })
        .collect()
}
pub fn generate(source: &Path, output: &Path) -> Result<()> {
    let workspace = compiler::compile(source, "rust")?;
    generate_rust(&workspace, source, output)
}
pub fn generate_rust(workspace: &compiler::Workspace, source: &Path, output: &Path) -> Result<()> {
    generate_rust_cached(workspace, source, output, None)
}
pub(crate) fn generate_rust_cached(
    workspace: &compiler::Workspace,
    source: &Path,
    output: &Path,
    cache: Option<&mut incremental::Bindings>,
) -> Result<()> {
    let stage = &workspace.stage;
    let packages: BTreeMap<String, String> = workspace
        .endpoints
        .iter()
        .map(|a| (a.source.clone(), a.package.clone()))
        .collect();
    let apis: Vec<Api> = workspace
        .endpoints
        .iter()
        .map(|a| a.runtime_api())
        .collect();
    let destination = output;
    let built = tempfile::tempdir()?;
    let output = built.path();
    let descriptor = output.join("descriptor.bin");
    use prost::Message;
    write_changed(&descriptor, &workspace.descriptor)?;
    let generate = |binding_output: &Path| -> Result<()> {
        prost_build::Config::new()
            .out_dir(binding_output)
            .include_file("messages.rs")
            .compile_fds(prost_types::FileDescriptorSet::decode(
                workspace.descriptor.as_slice(),
            )?)?;
        Ok(())
    };
    if let Some(cache) = cache {
        let roots: Vec<_> = workspace.files.values().cloned().collect();
        cache.generate(
            "rust:messages",
            &incremental::binding_key(workspace, &roots)?,
            output,
            generate,
        )?;
    } else {
        generate(output)?;
    }
    let pool = prost_reflect::DescriptorPool::decode(fs::read(&descriptor)?.as_slice())?;
    let mut generated = String::new();
    let mut routes = BTreeSet::new();
    let mut modules = BTreeSet::new();
    for api in &apis {
        if !routes.insert((api.method.clone(), api.path.clone())) {
            bail!("duplicate route {} {}", api.method, api.path);
        }
        if pool.get_message_by_name(&api.input).is_none()
            || pool.get_message_by_name(&api.output).is_none()
        {
            bail!("{}: missing in/out message", api.source);
        }
        if !modules.insert(api.source.clone()) {
            continue;
        }
        let module = api.source.trim_end_matches(".proto").replace('/', "__");
        let pkg = &packages[&api.source];
        let rust_pkg = pkg
            .split('.')
            .map(|s| s.to_snake_case())
            .collect::<Vec<_>>()
            .join("::");
        generated.push_str(&format!(
            r#"
#[allow(non_snake_case)]
pub mod {module} {{
    pub type In = crate::messages::{rust_pkg}::In;
    pub type Out = crate::messages::{rust_pkg}::Out;
    pub struct Endpoint;
    impl crate::Endpoint for Endpoint {{
        type In = In; type Out = Out;
        const PATH: &'static str = {path:?};
        const METHOD: &'static str = {method:?};
        const INPUT: &'static str = {input:?};
        const OUTPUT: &'static str = {out:?};
    }}
}}
"#,
            path = api.path,
            method = api.method,
            input = api.input,
            out = api.output
        ));
    }
    write_changed(&output.join("api.rs"), generated.as_bytes())?;
    write_changed(&output.join("api.json"), &serde_json::to_vec_pretty(&apis)?)?;
    let mut produced: Vec<String> = fs::read_dir(output)?
        .map(|e| e.map(|e| e.file_name().to_string_lossy().into_owned()))
        .collect::<std::io::Result<_>>()?;
    produced.sort();
    let old_manifest = destination.join(".generated.json");
    let old: Vec<String> = fs::read(&old_manifest)
        .ok()
        .and_then(|v| serde_json::from_slice(&v).ok())
        .unwrap_or_default();
    for name in &produced {
        write_changed(&destination.join(name), &fs::read(output.join(name))?)?;
    }
    for name in old {
        if !produced.contains(&name) && Path::new(&name).components().count() == 1 {
            let stale = destination.join(name);
            if stale.is_file() {
                fs::remove_file(stale)?;
            }
        }
    }
    write_changed(&old_manifest, &serde_json::to_vec(&produced)?)?;
    // Publish only after protoc and route validation succeed; remove deleted source artifacts.
    let target = source.join(".protos");
    let expected: BTreeSet<_> = files(stage.path())?
        .iter()
        .map(|p| p.strip_prefix(stage.path()).unwrap().to_owned())
        .collect();
    if target.exists() {
        for old in files(&target)? {
            if !expected.contains(old.strip_prefix(&target)?) {
                fs::remove_file(old)?;
            }
        }
    }
    for rel in expected {
        write_changed(&target.join(&rel), &fs::read(stage.path().join(rel))?)?;
    }
    Ok(())
}

pub mod metadata_edit;
pub mod navigation;
pub mod web;

pub mod environments;
