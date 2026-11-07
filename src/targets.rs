use crate::{
    compiler::{command, protoc, Config, Workspace},
    project::put,
};
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::{collections::BTreeMap, fs, path::Path, process::Command};

pub fn generate(cfg: &Config, out: &Config, work: &Workspace) -> Result<()> {
    match cfg.target.as_str() {
        "php" => php(cfg, out, work),
        "typescript" => typescript(cfg, out, work),
        "go" => go(cfg, out, work),
        "rust" => crate::generate_rust(work, work.stage.path(), &out.output),
        _ => unreachable!(),
    }
}
pub fn php_string(s: &str) -> String {
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
}
pub fn php_value(v: &Value) -> String {
    match v {
        Value::Null => "NULL".into(),
        Value::Bool(b) => b.to_string(),
        Value::String(s) => php_string(s),
        Value::Number(n) => n.to_string(),
        Value::Array(a) => format!(
            "array ({})",
            a.iter()
                .enumerate()
                .map(|(i, v)| format!("{i} => {},", php_value(v)))
                .collect::<Vec<_>>()
                .join(" ")
        ),
        Value::Object(m) => format!(
            "array ({})",
            m.iter()
                .map(|(k, v)| format!("{} => {},", php_string(k), php_value(v)))
                .collect::<Vec<_>>()
                .join(" ")
        ),
    }
}
fn php(cfg: &Config, out: &Config, w: &Workspace) -> Result<()> {
    let mut cmd = Command::new(protoc());
    cmd.current_dir(w.stage.path())
        .arg("-I.")
        .arg(format!("--php_out={}", out.output.display()))
        .args(w.files.values());
    command(&mut cmd)?;
    let mut routes = String::from("<?php\nreturn [\n");
    let mut seen = std::collections::BTreeSet::new();
    for a in &w.endpoints {
        let g = crate::studly(&a.group);
        let c = crate::studly(&a.controller);
        let namespace = format!("\\Proto\\Project\\{g}\\{}__{}", a.controller, a.action);
        let base = format!("/{}/{}", a.controller, a.action);
        let mut paths = vec![a.path.clone()];
        if a.meta.tags.iter().any(|t| t == "_uri_no_project") {
            paths.push(base);
        }
        for path in paths {
            anyhow::ensure!(seen.insert(path.clone()), "duplicate PHP alias {path}");
            routes.push_str(&format!("    [\\App\\Controllers\\{g}\\{c}::class, {}, {namespace}\\in::class, {namespace}\\out::class, {}, {}, {}, {}],\n",php_string(&a.action),php_string(&a.meta.title),php_string(&a.auth()),php_string(&path),php_value(&a.config)));
        }
    }
    routes.push_str("];\n");
    put(&out.output.join("Proto/cmd_list.php"), routes)?;
    let config_file = cfg.source.parent().unwrap().join("config.php");
    let config: Value = if config_file.exists() {
        serde_json::from_slice(&command(
            Command::new("php")
                .arg("-r")
                .arg("echo json_encode(require $argv[1], JSON_THROW_ON_ERROR);")
                .arg(&config_file),
        )?)?
    } else {
        json!({"new_control_flag":true,"export":[]})
    };
    let errors = crate::fields::errors(w);
    let info = json!({"struct_map":crate::fields::struct_map(w,true)?,"cmd_return_map":[],"error_list":errors,"menu_tree":{},"route_fix_config":{},"config":config});
    put(
        &out.output.join("info.json"),
        serde_json::to_vec_pretty(&info)?,
    )?;
    put(
        &out.output.join("cmd.json"),
        serde_json::to_vec_pretty(&crate::project::workspace_manifest(w, false))?,
    )?;
    let mut code = String::from("<?php\nreturn [\n");
    for (k, v) in errors.as_object().unwrap() {
        code.push_str(&format!(
            "    {k} => {},\n",
            php_string(v["desc"].as_str().unwrap())
        ));
    }
    code.push_str("];\n");
    put(&out.output.join("Proto/error.php"), code)?;
    Ok(())
}
fn typescript(cfg: &Config, out: &Config, w: &Workspace) -> Result<()> {
    let dest = out.output.join("proto");
    fs::create_dir_all(&dest)?;
    let plugin = cfg.root().join("node_modules/.bin/protoc-gen-ts_proto");
    command(Command::new(protoc()).current_dir(w.stage.path()).arg("-I.").arg(format!("--plugin=protoc-gen-ts_proto={}",plugin.display())).arg(format!("--ts_proto_out={}",dest.display())).arg("--ts_proto_opt=esModuleInterop=true,forceLong=string,outputJsonMethods=true,outputEncodeMethods=false,snakeToCamel=false").args(w.endpoints.iter().map(|a|&a.staged)))?;
    fn fix(dir: &Path) -> Result<()> {
        for e in fs::read_dir(dir)? {
            let p = e?.path();
            if p.is_dir() {
                fix(&p)?;
            } else if p.extension().is_some_and(|s| s == "ts") {
                let s = fs::read_to_string(&p)?;
                let mut s = s
                    .lines()
                    .filter(|s| !s.trim().starts_with("//   protoc "))
                    .collect::<Vec<_>>()
                    .join("\n")
                    + "\n";
                if !p.to_string_lossy().contains("/common/") {
                    if let Some(i) = s.find("createBaseout") {
                        let head = regex::Regex::new(r"\bin\b")?
                            .replace_all(&s[..i], "_in")
                            .into_owned();
                        s = head + &s[i..];
                    }
                }
                fs::write(&p, s)?;
            }
        }
        Ok(())
    }
    fix(&dest)?;
    let mut imports = vec![];
    let mut proto_imports = vec![];
    let mut definitions = vec![];
    let mut routes = vec![];
    let mut checks = vec![];
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for a in &w.endpoints {
        let cls = format!("{}__{}", a.group.replace('/', "__"), a.controller);
        let pv = format!(
            "{}__{}__{}",
            a.group.replace('/', "__"),
            a.controller,
            a.action
        );
        let proto_path = format!("@/gen/proto/{}/{}__{}", a.group, a.controller, a.action);
        let imp = format!(
            "import {cls} from '@/controllers/{}/{}';",
            a.group, a.controller
        );
        if !imports.contains(&imp) {
            imports.push(imp);
        }
        proto_imports.push(format!("import * as {pv} from '{proto_path}';"));
        definitions.push(format!(
            "import{{ _in as  {pv}_in, out as {pv}_out  }} from '{proto_path}';"
        ));
        groups
            .entry(cls.clone())
            .or_default()
            .push(a.action.clone());
        routes.push(format!("\n    {}: {{\n        ctrl:  {cls},\n        proto_in:  {pv}._in,\n        proto_out: {pv}.out,\n        action: {},\n        title:  {} ,\n        auth: {},\n    }}",json!(a.path),json!(a.action),json!(a.meta.title),json!(a.auth())));
        checks.push(format!("    ret=  await  (new {cls}).{}( {{ p_in: {pv}._in.fromJSON({{}}),   p_out:  {pv}.out.fromJSON({{}}),}} );",a.action));
    }
    let code = include_str!("../templates/cmd_map.ts.txt")
        .replace("{{IMPORTS}}", &imports.join("\n"))
        .replace("{{PROTO_IMPORTS}}", &proto_imports.join("\n"))
        .replace("{{ROUTES}}", &routes.join(",\n"))
        .replace("{{CHECKS}}", &checks.join("\n"));
    put(&out.output.join("cmd_map.ts"), code)?;
    let mut defs = definitions.join("\n") + "\n\n";
    for (cls, actions) in groups {
        defs.push_str(&format!("\nexport interface {cls} {{\n"));
        for action in actions {
            defs.push_str(&format!("\n    {action}:{{\n        p_in: {cls}__{action}_in,\n        p_out: {cls}__{action}_out,\n    }},\n"));
        }
        defs.push_str("\n};\n");
    }
    put(&out.output.join("proto_def.ts"), defs)?;
    put(
        &out.output.join("cmd.json"),
        serde_json::to_vec_pretty(&crate::project::workspace_manifest(w, true))?,
    )?;
    put(
        &out.output.join("proto_info.json"),
        serde_json::to_vec_pretty(&json!({"struct_map":crate::fields::struct_map(w,false)?}))?,
    )?;
    let mut errors = String::from("// error_code.ts\n\nconst ErrorCode = {\n");
    for (k, v) in crate::fields::errors(w).as_object().unwrap() {
        errors.push_str(&format!(
            "    {k}: {{ value: {k}, desc: {}, name : {} }},\n",
            json!(v["desc"]),
            json!(v["name"])
        ));
    }
    errors.push_str("};\nexport default ErrorCode;");
    put(&out.output.join("error_code.ts"), errors)?;
    put(
        &out.output
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("proto/proto.desc"),
        &w.descriptor,
    )?;
    Ok(())
}
fn go(cfg: &Config, out: &Config, w: &Workspace) -> Result<()> {
    let root = cfg.root();
    let module = fs::read_to_string(root.join("go.mod"))?
        .lines()
        .find_map(|l| l.strip_prefix("module "))
        .context("missing Go module")?
        .trim()
        .to_owned();
    let code = tempfile::tempdir()?;
    let plugin = root.join(".tools/protoc-gen-go");
    let mut cmd = Command::new(protoc());
    cmd.current_dir(w.stage.path())
        .arg("-I.")
        .arg(format!("--plugin=protoc-gen-go={}", plugin.display()))
        .arg(format!("--go_out={}", code.path().display()))
        .arg(format!("--go_opt=module={module}"));
    for (rel, dest) in &w.files {
        let api = w.endpoints.iter().find(|a| &a.source == rel);
        let (godir, pkg) = if let Some(a) = api {
            (
                format!("gen/proto/{}/{}__{}", a.group, a.controller, a.action),
                format!("{}{}", a.controller, a.action)
                    .chars()
                    .filter(|c| c.is_ascii_alphanumeric())
                    .collect::<String>(),
            )
        } else {
            ("gen/proto/common".into(), "common".into())
        };
        cmd.arg(format!("--go_opt=M{dest}={module}/{godir};{pkg}"));
    }
    cmd.args(w.files.values());
    command(&mut cmd)?;
    fn copy(from: &Path, to: &Path) -> Result<()> {
        for e in fs::read_dir(from)? {
            let e = e?;
            if e.path().is_dir() {
                copy(&e.path(), &to.join(e.file_name()))?;
            } else {
                put(&to.join(e.file_name()), fs::read(e.path())?)?;
            }
        }
        Ok(())
    }
    copy(&code.path().join("gen/proto"), &out.output)?;
    put(&out.output.join("descriptor.bin"), &w.descriptor)?;
    put(
        &out.output.join("api.json"),
        serde_json::to_vec_pretty(
            &w.endpoints
                .iter()
                .map(|a| a.runtime_api())
                .collect::<Vec<_>>(),
        )?,
    )?;
    let mut imports = vec![format!("\"{module}/pkg/httpcore\"")];
    let mut seen = std::collections::BTreeSet::new();
    let mut lines = vec![];
    for (i, a) in w.endpoints.iter().enumerate() {
        let alias = format!("api{i}");
        let ctrl = format!("controller_{}", a.group.replace('/', "_"));
        imports.push(format!(
            "{alias} \"{module}/gen/proto/{}/{}__{}\"",
            a.group, a.controller, a.action
        ));
        if seen.insert(ctrl.clone()) {
            imports.push(format!(
                "{ctrl} \"{module}/internal/controllers/{}\"",
                a.group
            ));
        }
        let method = if a.method().is_empty() {
            "POST"
        } else {
            a.method()
        };
        let num = a.meta.json_int64 == "JSON_INT64_NUMBER";
        lines.push(format!("httpcore.Register[*{alias}.In, *{alias}.Out](app, httpcore.Endpoint{{Path: {}, Method: {}, Auth: {}, Int64Number: {num}}}, (&{ctrl}.{}Controller{{}}).{})",json!(a.path),json!(method),json!(a.auth()),crate::studly(&a.controller),crate::studly(&a.action)));
    }
    let registry = out.registry.as_ref().context("Go registry required")?;
    let package = registry
        .parent()
        .unwrap()
        .file_name()
        .unwrap()
        .to_string_lossy();
    let text=format!("// Code generated by gen_proto.sh. DO NOT EDIT.\npackage {package}\nimport (\n{}\n)\nfunc Register(app *httpcore.App) {{\n{}\n}}\n",imports.join("\n"),lines.join("\n"));
    put(registry, text)?;
    command(Command::new("gofmt").arg("-w").arg(registry))?;
    Ok(())
}
