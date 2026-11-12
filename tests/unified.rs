use std::{fs, path::Path};
use workspace_proto_gen::{
    compiler::{self, Config},
    controller_edit, project,
};
fn put(root: &Path, name: &str, text: &str) {
    let p = root.join(name);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, text).unwrap();
}
fn fixture(target: &str) -> (tempfile::TempDir, Config) {
    let d = tempfile::tempdir().unwrap();
    put(
        d.path(),
        "proto/src/common/api_options.proto",
        compiler::SCHEMA,
    );
    put(
        d.path(),
        "proto/src/root/test__sum.proto",
        r#"syntax="proto3";
import "common/api_options.proto";
option(common.api)={title:"测试" auth:PUBLIC tags:"test" description:"中文 { } //__AUTH: SESSION\n```json\n{}\n```"};
message in { optional int32 value=1; }
message out { int64 total=1; }
"#,
    );
    put(d.path(),"proto/generator.toml",&format!("version=1\ntarget=\"{target}\"\nsource=\"src\"\noutput=\"../gen/proto\"\ncontrollers=\"../controllers\"\nregistry=\"../gen/controllers.rs\"\n"));
    let c = Config::load(&d.path().join("proto/generator.toml")).unwrap();
    (d, c)
}
#[test]
fn native_options_presence_and_default_http() {
    let (d, c) = fixture("rust");
    let w = compiler::compile(&c.source, "rust").unwrap();
    assert_eq!(w.endpoints[0].auth(), "public");
    assert_eq!(w.endpoints[0].method(), "");
    assert_eq!(w.endpoints[0].path, "/root/test/sum");
    assert!(w.endpoints[0].meta.description.contains("```json"));
    assert!(w
        .pool
        .get_message_by_name("proto.project.root.test__sum.in")
        .unwrap()
        .get_field_by_name("value")
        .unwrap()
        .supports_presence());
    assert!(w
        .pool
        .get_file_by_name("common/api_options.proto")
        .is_none());
    assert!(
        fs::read_to_string(d.path().join("proto/src/root/test__sum.proto"))
            .unwrap()
            .contains("option(common.api)")
    );
}
#[test]
fn invalid_metadata_fails_before_publishing() {
    for header in [
        "//__TITLE: old\n//__AUTH: public",
        "import \"common/api_options.proto\";option(common.api)={title:\"x\"};",
        "import \"common/api_options.proto\";option(common.api)={title:\"x\" auth:PRIVATE};",
    ] {
        let (d, c) = fixture("rust");
        put(
            d.path(),
            "proto/src/root/test__sum.proto",
            &format!("syntax=\"proto3\";\n{header}\nmessage in{{}}message out{{}}"),
        );
        assert!(compiler::compile(&c.source, "rust").is_err());
        assert!(!d.path().join("gen").exists());
    }
}
#[test]
fn php_check_is_read_only_and_detects_stale_output() {
    let (d, _) = fixture("php");
    let config = d.path().join("proto/generator.toml");
    project::run(&config, false).unwrap();
    let controller = d.path().join("controllers/Root/Test.php");
    let old = fs::read(&controller).unwrap();
    project::run(&config, true).unwrap();
    let generated = d.path().join("gen/proto/Proto/cmd_list.php");
    put(d.path(), "gen/proto/Proto/cmd_list.php", "stale");
    assert!(project::run(&config, true).is_err());
    assert_eq!(fs::read_to_string(&generated).unwrap(), "stale");
    assert_eq!(fs::read(&controller).unwrap(), old);
    project::run(&config, false).unwrap();
    project::run(&config, true).unwrap();
}
#[test]
fn controller_methods_ignore_comments_and_preserve_handwritten_body() {
    for target in ["php", "typescript"] {
        let (d, c) = fixture(target);
        let w = compiler::compile(&c.source, target).unwrap();
        let (rel, initial) = if target == "php" {
            ("controllers/Root/Test.php","<?php\nnamespace App\\Controllers\\Root;\nclass Test extends \\App\\Controllers\\Controller {\n// public function sum() {}\npublic function kept(){return '}中文';}\n}\n")
        } else {
            (
                "controllers/root/test.ts",
                "export default class test {\n// async sum() {}\nkept(){return '}中文';}\n}\n",
            )
        };
        put(d.path(), rel, initial);
        controller_edit::generate(&c, &w).unwrap();
        let p = d.path().join(rel);
        let first = fs::read_to_string(&p).unwrap();
        assert!(first.contains("return '}中文'"));
        assert!(first.contains(if target == "php" {
            "public function sum("
        } else {
            "async sum("
        }));
        controller_edit::generate(&c, &w).unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), first);
    }
}
#[test]
fn invalid_controller_rolls_back_all_generated_outputs() {
    let (d, _) = fixture("php");
    let config = d.path().join("proto/generator.toml");
    project::run(&config, false).unwrap();
    let old = fs::read(d.path().join("gen/proto/cmd.json")).unwrap();
    put(d.path(), "controllers/Root/Test.php", "<?php class Test {");
    assert!(project::run(&config, false).is_err());
    assert_eq!(fs::read(d.path().join("gen/proto/cmd.json")).unwrap(), old);
    assert_eq!(
        fs::read_to_string(d.path().join("controllers/Root/Test.php")).unwrap(),
        "<?php class Test {"
    );
}
#[test]
fn numeric_json_policy_is_rejected_and_64_bit_schema_is_string() {
    let (d, c) = fixture("rust");
    let file = d.path().join("proto/src/root/test__sum.proto");
    let original = fs::read_to_string(&file).unwrap();
    fs::write(
        &file,
        original.replace("auth:PUBLIC", "auth:PUBLIC json_int64:JSON_INT64_NUMBER"),
    )
    .unwrap();
    assert!(compiler::compile(&c.source, "rust").is_err());
    fs::write(&file, original).unwrap();
    let w = compiler::compile(&c.source, "rust").unwrap();
    let p = serde_json::from_value(serde_json::json!({"apifox_project_name":"test"})).unwrap();
    let doc = workspace_proto_gen::openapi::document(&w.endpoints[0], &w, &p).unwrap();
    assert_eq!(
        doc["components"]["schemas"]["proto.project.root.test__sum.out.response"]["properties"]
            ["total"]["type"],
        "string"
    );
}
#[test]
fn typescript_additional_methods_reuse_generated_type_imports() {
    let (d, c) = fixture("typescript");
    let mut w = compiler::compile(&c.source, "typescript").unwrap();
    controller_edit::generate(&c, &w).unwrap();
    let path = d.path().join("controllers/root/test.ts");
    let first = fs::read_to_string(&path).unwrap();
    let mut next = w.endpoints[0].clone();
    next.action = "next".into();
    w.endpoints.push(next);
    controller_edit::generate(&c, &w).unwrap();
    let result = fs::read_to_string(path).unwrap();
    assert!(result.contains("async next("));
    assert_eq!(result.matches("as __ProtoArgs").count(), 1);
    assert!(result.contains("async sum("));
    assert!(first.contains("async sum("));
}
#[test]
fn go_controller_import_aliases_and_existing_methods_are_preserved() {
    if std::process::Command::new("gofmt")
        .arg("-h")
        .output()
        .is_err()
    {
        eprintln!("Skipping Go controller test: gofmt is not installed");
        return;
    }
    let (d, c) = fixture("go");
    put(d.path(),"controllers/root/test.go","package root\nimport mycore \"go-core/pkg/httpcore\"\ntype Test struct{}\nfunc (Test) Kept(){ _ = \"}中文\" }\n");
    let w = compiler::compile(&c.source, "go").unwrap();
    controller_edit::generate(&c, &w).unwrap();
    let path = d.path().join("controllers/root/test.go");
    let result = fs::read_to_string(&path).unwrap();
    assert!(result.contains("ctx *mycore.Context"));
    assert!(result.contains("func (Test) Kept()"));
    controller_edit::generate(&c, &w).unwrap();
    assert_eq!(fs::read_to_string(path).unwrap(), result);
}
#[test]
fn go_controller_names_match_registry_and_navigation() {
    let Some(plugin) = std::env::var_os("PROTO_GEN_TEST_GO_PLUGIN") else {
        return;
    };
    let (d, c) = fixture("go");
    fs::create_dir_all(d.path().join(".tools")).unwrap();
    fs::copy(plugin, d.path().join(".tools/protoc-gen-go")).unwrap();
    put(d.path(), "go.mod", "module go-core\n\ngo 1.24.4\n");
    let config = d.path().join("proto/generator.toml");
    project::run(&config, false).unwrap();
    let controller = fs::read_to_string(c.controllers.join("root/test.go")).unwrap();
    assert!(controller.contains("type Test struct{}"));
    assert!(controller.contains("func (Test) Sum("));
    assert!(!controller.contains("TestController"));
    let registry = fs::read_to_string(c.registry.as_ref().unwrap()).unwrap();
    assert!(registry.contains("(&controller_root.Test{}).Sum"));
    let navigation: serde_json::Value =
        serde_json::from_slice(&fs::read(d.path().join("proto/navigation.json")).unwrap()).unwrap();
    assert_eq!(navigation["endpoints"][0]["controller"]["owner"], "Test");
    project::run(&config, true).unwrap();
}
#[test]
fn downloaded_binary_uses_its_bundled_compiler_without_source_paths() {
    for target in ["php", "rust"] {
        let (d, _) = fixture(target);
        let bundle = d.path().join("downloaded");
        fs::create_dir_all(&bundle).unwrap();
        let executable = bundle.join("proto-gen");
        fs::copy(env!("CARGO_BIN_EXE_proto-gen"), &executable).unwrap();
        let marker = bundle.join("compiler-used");
        let real = compiler::protoc();
        // Resolve PATH before the child deliberately replaces it with a rejecting compiler.
        let real = if real.components().count() == 1 && !real.is_absolute() {
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
                .map(|directory| directory.join(&real))
                .find(|candidate| candidate.is_file())
                .expect("protoc must be available on PATH")
        } else {
            real
        };
        let real = real.canonicalize().expect("cannot resolve the real protoc");
        let quote = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
        put(
            &bundle,
            "protoc",
            &format!(
                "#!/bin/sh\nprintf used >> {}\nexec {} \"$@\"\n",
                quote(marker.to_str().unwrap()),
                quote(real.to_str().unwrap())
            ),
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(bundle.join("protoc"), fs::Permissions::from_mode(0o755)).unwrap();
        }
        let reject = bundle.join("path-only");
        fs::create_dir_all(&reject).unwrap();
        put(
            &reject,
            "protoc",
            "#!/bin/sh\necho 'Unexpected PATH compiler' >&2\nexit 99\n",
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(reject.join("protoc"), fs::Permissions::from_mode(0o755)).unwrap();
        }
        let path = std::env::join_paths(
            std::iter::once(reject)
                .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
        )
        .unwrap();
        let result = std::process::Command::new(executable)
            .env("PATH", path)
            .env_remove("PROTOC")
            .arg("generate")
            .arg("--config")
            .arg(d.path().join("proto/generator.toml"))
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(
            marker.exists(),
            "portable binary did not use the compiler next to it"
        );
    }
}
