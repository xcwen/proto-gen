use crate::{
    compiler::{Config, Workspace},
    project::put,
};
use anyhow::{ensure, Context, Result};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
    process::Command,
};
use tree_sitter::{Node, Parser};
fn visit<'a>(node: Node<'a>, kind: &str, out: &mut Vec<Node<'a>>) {
    if node.kind() == kind {
        out.push(node);
    }
    let mut c = node.walk();
    for child in node.named_children(&mut c) {
        visit(child, kind, out);
    }
}
fn text<'a>(node: Node<'_>, source: &'a str) -> &'a str {
    &source[node.byte_range()]
}
fn tree(source: &str, target: &str) -> Result<tree_sitter::Tree> {
    let mut parser = Parser::new();
    let lang = match target {
        "php" => tree_sitter_php::LANGUAGE_PHP.into(),
        "typescript" => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        "go" => tree_sitter_go::LANGUAGE.into(),
        _ => unreachable!(),
    };
    parser.set_language(&lang)?;
    let t = parser
        .parse(source, None)
        .context("cannot parse controller")?;
    ensure!(
        !t.root_node().has_error(),
        "invalid {target} controller syntax; no output installed"
    );
    Ok(t)
}
pub fn generate(cfg: &Config, work: &Workspace) -> Result<()> {
    let mut groups: BTreeMap<String, Vec<&crate::compiler::Endpoint>> = BTreeMap::new();
    for a in &work.endpoints {
        groups
            .entry(format!("{}/{}", a.group, a.controller))
            .or_default()
            .push(a);
    }
    for (_, apis) in groups {
        match cfg.target.as_str() {
            "php" => php(&cfg.controllers, apis)?,
            "typescript" => typescript(&cfg.controllers, apis)?,
            "go" => go(&cfg.controllers, apis)?,
            _ => {}
        }
    }
    Ok(())
}
fn class_edit(root: &Path, apis: Vec<&crate::compiler::Endpoint>, php: bool) -> Result<()> {
    let a = apis[0];
    let cls = if php {
        crate::studly(&a.controller)
    } else {
        a.controller.clone()
    };
    let group = if php {
        crate::studly(&a.group)
    } else {
        a.group.clone()
    };
    let ext = if php { "php" } else { "ts" };
    let target = if php { "php" } else { "typescript" };
    let file = root.join(&group).join(format!("{cls}.{ext}"));
    let initial = if php {
        format!("<?php\ndeclare(strict_types=1);\nnamespace App\\Controllers\\{group};\nclass {cls} extends \\App\\Controllers\\Controller\n{{\n}}\n")
    } else {
        format!("import Controller from '@/controllers/controller';\nexport default class {cls} extends Controller {{\n}}\n")
    };
    let mut source = fs::read_to_string(&file).unwrap_or(initial);
    let ast = tree(&source, target)?;
    let mut classes = vec![];
    visit(ast.root_node(), "class_declaration", &mut classes);
    let class = classes
        .into_iter()
        .find(|n| {
            n.child_by_field_name("name")
                .is_some_and(|n| text(n, &source) == cls)
        })
        .context("controller class not found")?;
    let body = class
        .child_by_field_name("body")
        .context("class body missing")?;
    let mut names = BTreeSet::new();
    let mut cursor = body.walk();
    for m in body.named_children(&mut cursor) {
        if m.kind()
            == if php {
                "method_declaration"
            } else {
                "method_definition"
            }
        {
            if let Some(n) = m.child_by_field_name("name") {
                names.insert(text(n, &source).to_owned());
            }
        }
    }
    let mut imports = String::new();
    let mut append = String::new();
    for a in apis {
        if names.contains(&a.action) {
            continue;
        }
        if php {
            let proto = format!("\\Proto\\Project\\{group}\\{}__{}", a.controller, a.action);
            let title = a.meta.title.replace("*/", "* /");
            append.push_str(&format!("\n    /** {title} */\n    public function {}({proto}\\in $in, {proto}\\out &$out)\n    {{\n        return $this->output_err('自动生成代码，尚未实现');\n    }}\n",a.action));
        } else {
            let iface = format!("{}__{}", a.group.replace('/', "__"), a.controller);
            imports=format!("import type {{ {iface} as __ProtoArgs }} from '@/gen/proto_def';\nimport type {{ action_ret_t as __ActionResult }} from '@/utils/type_def';\n");
            ensure!(
                !source.contains("__ProtoArgs") && !source.contains("__ActionResult"),
                "controller uses reserved stub import aliases"
            );
            append.push_str(&format!("\n    async {}(p_args: __ProtoArgs[{}]): Promise<__ActionResult> {{\n        return this.output_err('自动生成代码，尚未实现');\n    }}\n",a.action,serde_json::json!(a.action)));
        }
    }
    if !append.is_empty() {
        source.insert_str(body.end_byte() - 1, &append);
        source.insert_str(0, &imports);
        tree(&source, target)?;
        put(&file, source)?;
    }
    Ok(())
}
fn php(root: &Path, a: Vec<&crate::compiler::Endpoint>) -> Result<()> {
    class_edit(root, a, true)
}
fn typescript(root: &Path, a: Vec<&crate::compiler::Endpoint>) -> Result<()> {
    class_edit(root, a, false)
}
fn go(root: &Path, apis: Vec<&crate::compiler::Endpoint>) -> Result<()> {
    let a = apis[0];
    let cls = format!("{}Controller", crate::studly(&a.controller));
    let file = root.join(&a.group).join(format!("{}.go", a.controller));
    let package = a.group.rsplit('/').next().unwrap();
    let mut source = fs::read_to_string(&file).unwrap_or_else(|_| format!("package {package}\n"));
    let ast = tree(&source, "go")?;
    let mut methods = vec![];
    visit(ast.root_node(), "method_declaration", &mut methods);
    let mut names = BTreeSet::new();
    for m in methods {
        if let Some(r) = m.child_by_field_name("receiver") {
            let mut ids = vec![];
            visit(r, "type_identifier", &mut ids);
            if ids.iter().any(|n| text(*n, &source) == cls) {
                if let Some(n) = m.child_by_field_name("name") {
                    names.insert(text(n, &source).to_owned());
                }
            }
        }
    }
    let missing: Vec<_> = apis
        .into_iter()
        .filter(|a| !names.contains(&crate::studly(&a.action)))
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    let mut specs = vec![];
    visit(ast.root_node(), "import_spec", &mut specs);
    let mut aliases = BTreeMap::new();
    for s in specs {
        if let Some(p) = s.child_by_field_name("path") {
            let path = text(p, &source).trim_matches('"').to_owned();
            let alias = s
                .child_by_field_name("name")
                .map(|n| text(n, &source).to_owned())
                .unwrap_or_else(|| path.rsplit('/').next().unwrap().into());
            aliases.insert(path, alias);
        }
    }
    let mut ids = vec![];
    visit(ast.root_node(), "identifier", &mut ids);
    let mut used: BTreeSet<String> = ids.iter().map(|n| text(*n, &source).to_owned()).collect();
    let mut imports = String::new();
    fn alias(
        path: &str,
        preferred: &str,
        map: &mut BTreeMap<String, String>,
        used: &mut BTreeSet<String>,
        imports: &mut String,
    ) -> Result<String> {
        if let Some(a) = map.get(path) {
            ensure!(
                a != "_" && a != ".",
                "unsupported blank/dot controller import"
            );
            return Ok(a.clone());
        }
        let mut a = preferred.to_owned();
        let mut i = 2;
        while used.contains(&a) {
            a = format!("{preferred}{i}");
            i += 1;
        }
        used.insert(a.clone());
        map.insert(path.into(), a.clone());
        imports.push_str(&format!("\nimport {a} {path:?}\n"));
        Ok(a)
    }
    let core = alias(
        "go-core/pkg/httpcore",
        "httpcore",
        &mut aliases,
        &mut used,
        &mut imports,
    )?;
    let mut additions = String::new();
    let mut types = vec![];
    visit(ast.root_node(), "type_spec", &mut types);
    if !types.iter().any(|n| {
        n.child_by_field_name("name")
            .is_some_and(|n| text(n, &source) == cls)
    }) {
        additions.push_str(&format!("\ntype {cls} struct{{}}\n"));
    }
    for a in missing {
        let method = crate::studly(&a.action);
        let p = alias(
            &format!(
                "go-core/gen/proto/{}/{}__{}",
                a.group, a.controller, a.action
            ),
            &format!("api{method}"),
            &mut aliases,
            &mut used,
            &mut imports,
        )?;
        additions.push_str(&format!("\nfunc ({cls}) {method}(ctx *{core}.Context, in *{p}.In) (*{p}.Out, error) {{\nreturn nil, {core}.NewError(1002, \"自动生成代码，尚未实现\")\n}}\n"));
    }
    let mut packages = vec![];
    visit(ast.root_node(), "package_clause", &mut packages);
    let end = packages.first().context("missing Go package")?.end_byte();
    source.insert_str(end, &imports);
    source.push_str(&additions);
    tree(&source, "go")?;
    put(&file, source)?;
    crate::compiler::command(Command::new("gofmt").arg("-w").arg(&file))?;
    Ok(())
}
