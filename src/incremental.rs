//! Content-based generation cache. Cached bindings are verified against installed files;
//! the state is published only as part of a successful project installation.
use crate::compiler::{Config, Workspace};
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
};

pub type Fingerprint = BTreeMap<PathBuf, String>;
pub fn digest(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}
fn relative(path: &Path) -> bool {
    !path.as_os_str().is_empty() && path.components().all(|c| matches!(c, Component::Normal(_)))
}
fn tree(path: &Path, result: &mut Fingerprint) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    if path.is_file() {
        result.insert(path.to_owned(), digest(&fs::read(path)?));
        return Ok(());
    }
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if !entry.file_name().to_string_lossy().starts_with('.') {
            tree(&entry.path(), result)?;
        }
    }
    Ok(())
}
fn optional(path: &Path, result: &mut Fingerprint) -> Result<()> {
    result.insert(
        path.to_owned(),
        if path.is_file() {
            digest(&fs::read(path)?)
        } else {
            "missing".into()
        },
    );
    Ok(())
}
fn executable(path: &Path) -> PathBuf {
    if path.components().count() > 1 || path.is_absolute() {
        return path.to_owned();
    }
    std::env::var_os("PATH")
        .and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|p| p.join(path))
                .find(|p| p.is_file())
        })
        .unwrap_or_else(|| path.to_owned())
}
pub(crate) fn go_plugin(cfg: &Config) -> Result<PathBuf> {
    let local = cfg.root().join(".tools/protoc-gen-go");
    Ok(if local.is_file() {
        local
    } else {
        std::env::current_exe()?
            .parent()
            .unwrap()
            .join("protoc-gen-go")
    })
}
pub fn inputs(cfg: &Config, config: &Path) -> Result<Fingerprint> {
    let mut result = BTreeMap::new();
    fn source_tree(path: &Path, result: &mut Fingerprint) -> Result<()> {
        if path.is_file() {
            return optional(path, result);
        }
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            // Rust's legacy prepared sources are generated, not inputs. Hidden Markdown
            // files may still be referenced by description_file and must be tracked.
            if entry.file_name() != ".protos" {
                source_tree(&entry.path(), result)?;
            }
        }
        Ok(())
    }
    source_tree(&cfg.source, &mut result)?;
    optional(config, &mut result)?;
    optional(&std::env::current_exe()?, &mut result)?;
    optional(&executable(&crate::compiler::protoc()), &mut result)?;
    let proto = cfg.source.parent().unwrap();
    for name in ["config.json", "config.php"] {
        optional(&proto.join(name), &mut result)?;
    }
    let settings = crate::settings::Settings::load(proto)?;
    optional(
        &proto.join(
            settings
                .enum_catalog
                .as_deref()
                .unwrap_or("enum_catalog.json"),
        ),
        &mut result,
    )?;
    match cfg.target.as_str() {
        "typescript" => {
            let root = cfg.root();
            let plugin = root.join("node_modules/.bin/protoc-gen-ts_proto");
            optional(&plugin, &mut result)?;
            if let Ok(real) = plugin.canonicalize() {
                // Include the plugin implementation, not just the npm launcher.
                if let Some(package) = real.parent() {
                    tree(package, &mut result)?;
                }
            }
            for name in [
                "package.json",
                "package-lock.json",
                "pnpm-lock.yaml",
                "yarn.lock",
            ] {
                optional(&root.join(name), &mut result)?;
            }
            optional(&executable(Path::new("node")), &mut result)?;
        }
        "go" => {
            optional(&cfg.root().join("go.mod"), &mut result)?;
            optional(&go_plugin(cfg)?, &mut result)?;
            optional(&executable(Path::new("gofmt")), &mut result)?;
        }
        "rust" => {
            optional(&executable(Path::new("rustfmt")), &mut result)?;
        }
        _ => {}
    }
    // Environment values are hashed, never persisted as plaintext.
    for name in ["PATH", "PROTOC", "NODE_OPTIONS", "NODE_PATH"] {
        result.insert(
            PathBuf::from(format!("env:{name}")),
            digest(
                std::env::var_os(name)
                    .unwrap_or_default()
                    .as_encoded_bytes(),
            ),
        );
    }
    Ok(result)
}

#[derive(Default, Serialize, Deserialize)]
pub struct State {
    pub version: u32,
    pub inputs: Fingerprint,
    pub observed: Fingerprint,
    pub bindings: BTreeMap<String, Binding>,
    pub interfaces: usize,
}
impl State {
    pub fn load(path: &Path) -> Option<Self> {
        let state: Self = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
        (state.version == 1
            && !state.observed.is_empty()
            && state.observed.keys().all(|p| relative(p))
            && state
                .bindings
                .values()
                .all(|b| b.outputs.keys().all(|p| relative(p))))
        .then_some(state)
    }
    pub fn unchanged(&self, inputs: &Fingerprint, root: &Path, cfg: &Config) -> Result<bool> {
        Ok(self.inputs == *inputs && self.observed == observed(root, cfg, self.observed.keys())?)
    }
}
pub fn observed<'a>(
    root: &Path,
    cfg: &Config,
    paths: impl IntoIterator<Item = &'a PathBuf>,
) -> Result<Fingerprint> {
    let mut result = BTreeMap::new();
    for path in paths {
        ensure!(relative(path), "invalid cached project path");
        optional(&root.join(path), &mut result)?;
    }
    // Detect new/deleted/edited handwritten controllers as well as missing generated files.
    tree(&cfg.controllers, &mut result)?;
    Ok(result
        .into_iter()
        .map(|(p, hash)| (p.strip_prefix(root).unwrap_or(&p).to_owned(), hash))
        .collect())
}
#[derive(Default, Serialize, Deserialize)]
pub struct Binding {
    key: String,
    outputs: Fingerprint,
}
pub struct Bindings {
    installed: PathBuf,
    old: BTreeMap<String, Binding>,
    pub next: BTreeMap<String, Binding>,
    pub compiled: usize,
    pub reused: usize,
    tool_key: String,
}
impl Bindings {
    pub fn new(cfg: &Config, old: Option<State>, inputs: &Fingerprint) -> Result<Self> {
        // Source/config changes are handled by the per-unit key; tool changes invalidate every unit.
        let proto = cfg.source.parent().unwrap();
        let settings = crate::settings::Settings::load(proto)?;
        let enum_catalog = proto.join(
            settings
                .enum_catalog
                .as_deref()
                .unwrap_or("enum_catalog.json"),
        );
        let tools: BTreeMap<_, _> = inputs
            .iter()
            .filter(|(p, _)| {
                !p.starts_with(&cfg.source)
                    && p.as_path() != proto.join("config.json")
                    && p.as_path() != enum_catalog
            })
            .collect();
        Ok(Self {
            installed: cfg.output.clone(),
            old: old.map(|s| s.bindings).unwrap_or_default(),
            next: BTreeMap::new(),
            compiled: 0,
            reused: 0,
            tool_key: digest(&serde_json::to_vec(&tools)?),
        })
    }
    pub fn generate(
        &mut self,
        id: &str,
        key: &str,
        output: &Path,
        generate: impl FnOnce(&Path) -> Result<()>,
    ) -> Result<()> {
        let key = digest(&serde_json::to_vec(&(&self.tool_key, key))?);
        if let Some(old) = self.old.get(id) {
            let mut valid = old.key == key;
            for (path, hash) in &old.outputs {
                valid &= relative(path)
                    && fs::read(self.installed.join(path))
                        .is_ok_and(|bytes| digest(&bytes) == *hash);
            }
            if valid {
                for path in old.outputs.keys() {
                    let bytes = fs::read(self.installed.join(path))?;
                    if output.join(path).exists() {
                        ensure!(
                            fs::read(output.join(path))? == bytes,
                            "conflicting shared binding output: {}",
                            path.display()
                        );
                    }
                    crate::project::put(&output.join(path), bytes)?;
                }
                self.next.insert(
                    id.into(),
                    Binding {
                        key,
                        outputs: old.outputs.clone(),
                    },
                );
                self.reused += 1;
                return Ok(());
            }
        }
        let temp = tempfile::tempdir()?;
        generate(temp.path())?;
        let mut files = BTreeMap::new();
        tree(temp.path(), &mut files)?;
        let mut outputs = BTreeMap::new();
        for path in files.keys() {
            let rel = path.strip_prefix(temp.path())?;
            let bytes = fs::read(path)?;
            if output.join(rel).exists() {
                ensure!(
                    fs::read(output.join(rel))? == bytes,
                    "conflicting shared binding output: {}",
                    rel.display()
                );
            }
            crate::project::put(&output.join(rel), &bytes)?;
            outputs.insert(rel.to_owned(), digest(&bytes));
        }
        self.next.insert(id.into(), Binding { key, outputs });
        self.compiled += 1;
        Ok(())
    }
}
/// Hash the exact stripped compiler inputs and their transitive imports. Metadata-only
/// edits can update global indexes without invoking message plugins.
pub fn binding_key(work: &Workspace, roots: &[String]) -> Result<String> {
    fn visit(work: &Workspace, name: &str, files: &mut Fingerprint) -> Result<()> {
        let path = PathBuf::from(name);
        if files.contains_key(&path) {
            return Ok(());
        }
        files.insert(path, digest(&fs::read(work.stage.path().join(name))?));
        if let Some(file) = work.pool.get_file_by_name(name) {
            for dep in &file.file_descriptor_proto().dependency {
                visit(work, dep, files)?;
            }
        }
        Ok(())
    }
    let mut files = BTreeMap::new();
    for root in roots {
        visit(work, root, &mut files)?;
    }
    Ok(digest(&serde_json::to_vec(&files)?))
}
