use crate::compiler::{Config, Workspace};
use anyhow::{ensure, Context, Result};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

pub fn clean(path: &Path) -> PathBuf {
    let mut p = PathBuf::new();
    for c in path.components() {
        if c == std::path::Component::ParentDir {
            p.pop();
        } else {
            p.push(c);
        }
    }
    p
}
pub fn put(path: &Path, data: impl AsRef<[u8]>) -> Result<()> {
    fs::create_dir_all(path.parent().unwrap())?;
    fs::write(path, data)?;
    Ok(())
}
fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    if !from.exists() {
        return Ok(());
    }
    for e in fs::read_dir(from)? {
        let e = e?;
        let dest = to.join(e.file_name());
        if e.file_type()?.is_dir() {
            copy_tree(&e.path(), &dest)?
        } else {
            put(&dest, fs::read(e.path())?)?;
        }
    }
    Ok(())
}
pub fn run(config_file: &Path, check: bool) -> Result<()> {
    let mut cfg = Config::load(config_file)?;
    cfg.source = clean(&cfg.source);
    cfg.output = clean(&cfg.output);
    cfg.controllers = clean(&cfg.controllers);
    cfg.registry = cfg.registry.as_ref().map(|p| clean(p));
    let root = cfg.root().canonicalize()?;
    let work = crate::compiler::compile(&cfg.source, &cfg.target)?;
    let temp = tempfile::tempdir()?;
    let mirror = temp.path().join("project");
    fs::create_dir_all(&mirror)?;
    let mut stage_cfg = cfg.clone();
    stage_cfg.output = mirror.join(
        cfg.output
            .strip_prefix(&root)
            .context("output must be in project")?,
    );
    stage_cfg.controllers = mirror.join(
        cfg.controllers
            .strip_prefix(&root)
            .context("controllers must be in project")?,
    );
    stage_cfg.registry = cfg
        .registry
        .as_ref()
        .map(|p| -> Result<PathBuf> { Ok(mirror.join(p.strip_prefix(&root)?)) })
        .transpose()?;
    copy_tree(&cfg.controllers, &stage_cfg.controllers)?;
    fs::create_dir_all(&stage_cfg.output)?;
    crate::targets::generate(&cfg, &stage_cfg, &work)?;
    if cfg.target == "rust" {
        crate::generate_controllers(
            &stage_cfg.output,
            &stage_cfg.controllers,
            stage_cfg
                .registry
                .as_ref()
                .context("Rust registry required")?,
        )?;
    } else {
        crate::controller_edit::generate(&stage_cfg, &work)?;
    }
    let mut planned = BTreeMap::new();
    collect(&mirror, &mirror, &mut planned)?;
    // The stage also contains unchanged controller copies; only changed files are written.
    let manifest = cfg
        .source
        .parent()
        .unwrap()
        .join(".proto-gen-manifest.json");
    let old: Vec<String> = fs::read(&manifest)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let generated: Vec<String> = planned
        .keys()
        .filter(|p| !root.join(p).starts_with(&cfg.controllers))
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .collect();
    let expected: BTreeSet<_> = generated.iter().cloned().collect();
    let stale: Vec<_> = old.iter().filter(|p| !expected.contains(*p)).collect();
    let changes: Vec<_> = planned
        .iter()
        .filter(|(p, b)| fs::read(root.join(p)).ok().as_ref() != Some(*b))
        .collect();
    if check {
        ensure!(
            changes.is_empty() && stale.is_empty(),
            "generated output is stale: {} changed, {} removed",
            changes.len(),
            stale.len()
        );
        println!(
            "{}: {} interfaces up to date",
            cfg.target,
            work.endpoints.len()
        );
        return Ok(());
    }
    // Roll back installed files if any filesystem operation fails.
    let mut backup = BTreeMap::new();
    for (p, _) in &changes {
        backup.insert(root.join(p), fs::read(root.join(p)).ok());
    }
    for p in &stale {
        let path = clean(&root.join(p));
        ensure!(
            path.starts_with(&cfg.output),
            "refusing to prune outside generated output"
        );
        backup.insert(path.clone(), fs::read(path).ok());
    }
    let install = (|| -> Result<()> {
        for (p, b) in &changes {
            put(&root.join(p), b)?;
        }
        for p in stale {
            let path = root.join(p);
            if path.exists() {
                fs::remove_file(path)?;
            }
        }
        put(&manifest, serde_json::to_vec_pretty(&generated)?)?;
        Ok(())
    })();
    if let Err(e) = install {
        for (p, data) in backup {
            if let Some(data) = data {
                put(&p, data)?;
            } else if p.exists() {
                fs::remove_file(p)?;
            }
        }
        return Err(e);
    }
    println!(
        "{}: {} interfaces, {} changed files",
        cfg.target,
        work.endpoints.len(),
        changes.len()
    );
    Ok(())
}
fn collect(base: &Path, dir: &Path, map: &mut BTreeMap<PathBuf, Vec<u8>>) -> Result<()> {
    for e in fs::read_dir(dir)? {
        let e = e?;
        if e.file_type()?.is_dir() {
            collect(base, &e.path(), map)?;
        } else {
            map.insert(e.path().strip_prefix(base)?.to_owned(), fs::read(e.path())?);
        }
    }
    Ok(())
}
pub fn workspace_manifest(work: &Workspace, node: bool) -> serde_json::Value {
    let mut tags = vec![];
    let mut owners = vec![];
    for a in &work.endpoints {
        for t in &a.meta.tags {
            if !tags.contains(t) {
                tags.push(t.clone());
            }
        }
        for t in &a.meta.maintainers {
            if !owners.contains(t) {
                owners.push(t.clone());
            }
        }
    }
    if node {
        tags.sort_by_key(|s| s.to_lowercase());
        owners.sort_by_key(|s| s.to_lowercase());
    }
    serde_json::json!({"tag_list":tags,"maintainer_list":owners,"cmd_list":work.endpoints.iter().map(|a|a.manifest()).collect::<Vec<_>>()})
}
