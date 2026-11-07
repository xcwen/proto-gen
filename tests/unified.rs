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
