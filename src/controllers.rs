use crate::{write_changed, Api};
use anyhow::{Context, Result};
use heck::{ToSnakeCase, ToUpperCamelCase};
use proc_macro2::LineColumn;
use std::io::Write;
use std::process::{Command, Stdio};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};
use syn::{ImplItem, Item, Type};

struct Action {
    method: String,
    endpoint: String,
    title: String,
    input: String,
    fields: Vec<String>,
}
struct Controller {
    class: String,
    actions: Vec<Action>,
}
fn relative_path(from: &Path, to: &Path) -> Result<String> {
    let from = std::path::absolute(from)?;
    let to = std::path::absolute(to)?;
    let from: Vec<_> = from.components().collect();
    let to: Vec<_> = to.components().collect();
    let shared = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    anyhow::ensure!(
        shared > 0,
        "controller and registry must share a filesystem root"
    );
    let mut relative = std::path::PathBuf::new();
    for _ in shared..from.len() {
        relative.push("..");
    }
    for component in &to[shared..] {
        relative.push(component.as_os_str());
    }
    Ok(relative
        .to_str()
        .context("non-UTF8 controller path")?
        .replace('\\', "/"))
}
fn identifier(name: &str) -> Result<String> {
    if syn::parse_str::<syn::Ident>(name).is_ok() {
        return Ok(name.into());
    }
    let raw = format!("r#{name}");
    syn::parse_str::<syn::Ident>(&raw)
        .with_context(|| format!("unsupported Rust identifier: {name}"))?;
    Ok(raw)
}
fn offset(text: &str, position: LineColumn) -> Result<usize> {
    let start = text
        .split_inclusive('\n')
        .take(position.line.saturating_sub(1))
        .map(str::len)
        .sum::<usize>();
    let line = text
        .split_inclusive('\n')
        .nth(position.line.saturating_sub(1))
        .context("invalid source line")?;
    let column = line
        .char_indices()
        .nth(position.column)
        .map(|(offset, _)| offset)
        .unwrap_or(line.len());
    let result = start + column;
    anyhow::ensure!(
        text.is_char_boundary(result),
        "invalid source insertion position"
    );
    Ok(result)
}
fn format_rust(text: &str) -> Result<String> {
    let mut child = Command::new("rustfmt")
        .args(["--emit", "stdout", "--edition", "2021"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("rustfmt is required for controller generation")?;
    child
        .stdin
        .take()
        .context("missing rustfmt stdin")?
        .write_all(text.as_bytes())?;
    let output = child.wait_with_output()?;
    anyhow::ensure!(
        output.status.success(),
        "rustfmt failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)?)
}
fn controller_name(ty: &Type) -> Option<String> {
    if let Type::Path(ty) = ty {
        ty.path.get_ident().map(ToString::to_string)
    } else {
        None
    }
}
fn method(action: &Action) -> Result<String> {
    let endpoint = &action.endpoint;
    let name = identifier(&action.method)?;
    let mut text = String::new();
    for line in action.title.lines() {
        text.push_str(&format!("    /// {line}\n"));
    }
    text.push_str(&format!("    pub async fn {name}(\n        _ctx: core_http::RequestContext,\n        input: core_proto::api::{endpoint}::In,\n    ) -> Result<core_proto::api::{endpoint}::Out, core_http::AppError> {{\n"));
    text.push_str("        let _ = &input;\n");
    for field in &action.fields {
        let local = format!("_{}", field.trim_start_matches("r#"));
        text.push_str(&format!("        let {local} = input.{field};\n"));
    }
    let error = format!(
        "自动生成代码，[{}]还未实现！",
        action.input.trim_end_matches(".in")
    );
    text.push_str(&format!(
        "        Err(core_http::AppError::new(1002, {error:?}))\n    }}\n"
    ));
    Ok(text)
}
fn update(original: &str, controller: &Controller) -> Result<String> {
    let parsed = syn::parse_file(original).context("controller contains invalid Rust syntax")?;
    anyhow::ensure!(
        parsed
            .items
            .iter()
            .any(|item| matches!(item, Item::Struct(s) if s.ident == controller.class)),
        "missing controller struct {}",
        controller.class
    );
    let mut names = BTreeSet::new();
    let mut insertion = None;
    for item in &parsed.items {
        if let Item::Impl(implementation) = item {
            if implementation.trait_.is_none()
                && controller_name(&implementation.self_ty).as_deref() == Some(&controller.class)
            {
                if insertion.is_none() {
                    insertion = Some(offset(
                        original,
                        implementation.brace_token.span.close().start(),
                    )?);
                }
                for item in &implementation.items {
                    if let ImplItem::Fn(method) = item {
                        names.insert(method.sig.ident.to_string());
                    }
                }
            }
        }
    }
    let mut additions = String::new();
    for action in &controller.actions {
        if !names.contains(&identifier(&action.method)?) {
            additions.push_str(&method(action)?);
        }
    }
    if additions.is_empty() {
        return Ok(original.into());
    }
    let formatted = format_rust(&format!("impl {} {{\n{additions}}}\n", controller.class))?;
    let body_start = formatted.find('{').context("missing generated impl body")? + 1;
    let body_end = formatted.rfind('}').context("missing generated impl end")?;
    let additions = &formatted[body_start..body_end];
    let result = if let Some(at) = insertion {
        let prefix = &original[..at];
        let separator = if prefix.ends_with('\n') { "" } else { "\n" };
        format!(
            "{prefix}{separator}{}{}",
            additions.trim_start_matches('\n'),
            &original[at..]
        )
    } else {
        format!("{original}\n{formatted}")
    };
    syn::parse_file(&result).context("generated invalid controller")?;
    Ok(result)
}
/// Generate editable controller skeletons and a replaceable module/route registry.
/// Existing source is parsed to locate inherent impls; only absent methods are added.
pub fn generate_controllers(output: &Path, root: &Path, registry_path: &Path) -> Result<()> {
    let apis: Vec<Api> = serde_json::from_slice(&fs::read(output.join("api.json"))?)?;
    let mut controllers: BTreeMap<String, Controller> = BTreeMap::new();
    let mut modules: BTreeMap<String, String> = BTreeMap::new();
    let mut registration = String::new();
    let mut methods = BTreeSet::new();
    for api in apis {
        let stem = api
            .source
            .strip_suffix(".proto")
            .context("invalid protocol filename")?;
        let (project, name) = stem.rsplit_once('/').context("missing project directory")?;
        let (ctrl, action) = name
            .split_once("__")
            .context("missing controller/action separator")?;
        let project: Vec<_> = project.split('/').map(ToSnakeCase::to_snake_case).collect();
        let ctrl = ctrl.to_snake_case();
        let action = action.to_snake_case();
        anyhow::ensure!(
            !ctrl.is_empty() && !action.is_empty() && project.iter().all(|p| !p.is_empty()),
            "invalid controller name: {}",
            api.source
        );
        for part in project.iter().chain([&ctrl, &action]) {
            identifier(part)?;
        }
        let class = ctrl.to_upper_camel_case();
        identifier(&class)?;
        let path = format!("{}/{}.rs", project.join("/"), ctrl);
        anyhow::ensure!(
            methods.insert((path.clone(), action.clone())),
            "duplicate controller method for {}",
            api.source
        );
        let module = format!("{}_{}", project.join("_"), ctrl);
        if let Some(previous) = modules.insert(module.clone(), path.clone()) {
            anyhow::ensure!(previous == path, "controller module collision: {module}");
        }
        let endpoint = stem.replace('/', "__");
        identifier(&endpoint)?;
        let rust_path = format!(
            "{}::{}::{}",
            identifier(&module)?,
            class,
            identifier(&action)?
        );
        registration.push_str(&format!(
            "    app.register_action::<core_proto::api::{endpoint}::Endpoint, _, _>({})?;\n",
            rust_path
        ));
        // Inspect prost's actual Rust struct fields: handles keywords and oneof groups.
        let package = api
            .input
            .strip_suffix(".in")
            .context("invalid input message")?;
        let generated_name = package
            .split('.')
            .map(ToSnakeCase::to_snake_case)
            .collect::<Vec<_>>()
            .join(".");
        let generated = syn::parse_file(&fs::read_to_string(
            output.join(format!("{generated_name}.rs")),
        )?)?;
        let input = generated
            .items
            .iter()
            .find_map(|item| {
                if let Item::Struct(s) = item {
                    if s.ident == "In" {
                        Some(s)
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .context("missing generated In struct")?;
        let fields = input
            .fields
            .iter()
            .filter_map(|field| field.ident.as_ref().map(ToString::to_string))
            .collect();
        controllers
            .entry(path)
            .or_insert_with(|| Controller {
                class,
                actions: vec![],
            })
            .actions
            .push(Action {
                method: action,
                endpoint,
                title: api.title,
                input: api.input,
                fields,
            });
    }
    let parameter = if registration.is_empty() {
        "_app"
    } else {
        "app"
    };
    let mut declarations = String::new();
    for (module, path) in modules {
        let path = relative_path(
            registry_path
                .parent()
                .context("registry requires a parent directory")?,
            &root.join(path),
        )?;
        declarations.push_str(&format!(
            "#[path = {path:?}]\npub mod {};\n",
            identifier(&module)?
        ));
    }
    let registry = format!("// @generated by gen_proto.sh. Edit controller files; do not edit this registry.\n{}\npub fn register({parameter}: &mut core_http::HttpApp) -> anyhow::Result<()> {{\n{registration}    Ok(())\n}}\n", declarations);
    syn::parse_file(&registry)?;
    let registry = format_rust(&registry)?;
    let mut writes = vec![];
    // Validate every existing controller before publishing any source changes.
    for (path, controller) in controllers {
        let path = root.join(path);
        let is_new = !path.exists();
        let original = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind()==std::io::ErrorKind::NotFound => format!("// Created by gen_proto.sh. This file is editable; existing methods are preserved.\npub struct {};\n\nimpl {} {{\n}}\n",controller.class,controller.class),
            Err(error) => return Err(error.into()),
        };
        let changed = update(&original, &controller)
            .with_context(|| format!("cannot update {}", path.display()))?;
        let changed = if is_new {
            format_rust(&changed)?
        } else {
            changed
        };
        writes.push((path, changed));
    }
    // Removed protocols disappear from the registry; their business files remain untouched.
    for (path, text) in writes {
        write_changed(&path, text.as_bytes())?;
    }
    write_changed(registry_path, registry.as_bytes())?;
    Ok(())
}
