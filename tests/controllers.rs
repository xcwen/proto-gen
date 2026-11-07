use std::{fs, path::Path};
fn put(root: &Path, path: &str, text: &str) {
    let file = root.join(path);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(file, text).unwrap();
}
fn protocol(root: &Path, name: &str) {
    put(
        root,
        "common/api_options.proto",
        workspace_proto_gen::compiler::SCHEMA,
    );
    put(root, name, "syntax=\"proto3\";\nimport \"common/api_options.proto\";\noption(common.api)={title:\"测试接口\" auth:PUBLIC};\nmessage in { optional int32 id=1; string type=2; oneof selection {string name=3; int32 count=4;} }\nmessage out {}\n");
}
fn generate(src: &Path, out: &Path, controllers: &Path) {
    workspace_proto_gen::generate(src, out).unwrap();
    workspace_proto_gen::generate_controllers(out, controllers, &out.join("controllers.rs"))
        .unwrap();
}
#[test]
fn creates_grouped_controller_field_reads_and_automatic_registration() {
    let temp = tempfile::tempdir().unwrap();
    let src = temp.path().join("src");
    let out = temp.path().join("out");
    let controllers = temp.path().join("controllers");
    protocol(&src, "my_project/user_profile__get_info.proto");
    protocol(&src, "my_project/user_profile__update.proto");
    generate(&src, &out, &controllers);
    let file = controllers.join("my_project/user_profile.rs");
    let source = fs::read_to_string(&file).unwrap();
    assert!(source.contains("pub struct UserProfile;"));
    assert!(source.contains("pub async fn get_info("));
    assert!(source.contains("pub async fn update("));
    assert!(source.contains("let _id = input.id;"));
    assert!(source.contains("let _type = input.r#type;"));
    assert!(source.contains("let _selection = input.selection;"));
    assert!(!source.contains("input.name")); // oneof arms aren't Rust struct fields
    assert!(source.contains("/// 测试接口"));
    assert!(source.contains("还未实现"));
    let registry = fs::read_to_string(out.join("controllers.rs")).unwrap();
    assert!(registry.contains("pub mod my_project_user_profile"));
    assert!(registry.contains("my_project_user_profile::UserProfile::get_info"));
    workspace_proto_gen::generate_controllers(&out, &controllers, &out.join("controllers.rs"))
        .unwrap();
    assert_eq!(source, fs::read_to_string(file).unwrap());
    assert_eq!(
        registry,
        fs::read_to_string(out.join("controllers.rs")).unwrap()
    );
}
#[test]
fn appends_only_missing_methods_preserving_comments_strings_and_other_impls() {
    let temp = tempfile::tempdir().unwrap();
    let src = temp.path().join("src");
    let out = temp.path().join("out");
    let controllers = temp.path().join("controllers");
    for method in ["first", "second", "third"] {
        protocol(&src, &format!("root/test__{method}.proto"));
    }
    let prefix = "// handwritten header\npub struct Test;\nimpl Test { pub async fn first() { let _ = \"保留中文 }\"; } ";
    let suffix = "}\n// pub async fn third() { fake method in comment }\nimpl Test { pub async fn second() {} }\nconst KEEP: &str = \"} unchanged\";\n";
    put(&controllers, "root/test.rs", &format!("{prefix}{suffix}"));
    generate(&src, &out, &controllers);
    let changed = fs::read_to_string(controllers.join("root/test.rs")).unwrap();
    assert!(changed.starts_with(prefix));
    assert!(changed.ends_with(suffix));
    assert_eq!(changed.matches("pub async fn first(").count(), 1);
    assert_eq!(changed.matches("pub async fn second(").count(), 1);
    assert_eq!(changed.matches("pub async fn third(").count(), 2); // comment plus actual method
    syn::parse_file(&changed).unwrap();
    workspace_proto_gen::generate_controllers(&out, &controllers, &out.join("controllers.rs"))
        .unwrap();
    assert_eq!(
        changed,
        fs::read_to_string(controllers.join("root/test.rs")).unwrap()
    );
}
#[test]
fn invalid_existing_source_aborts_all_controller_writes() {
    let temp = tempfile::tempdir().unwrap();
    let src = temp.path().join("src");
    let out = temp.path().join("out");
    let controllers = temp.path().join("controllers");
    protocol(&src, "root/a_new__create.proto");
    protocol(&src, "root/z_bad__update.proto");
    put(
        &controllers,
        "root/z_bad.rs",
        "pub struct ZBad; impl ZBad {",
    );
    put(&out, "controllers.rs", "// existing registry\n");
    workspace_proto_gen::generate(&src, &out).unwrap();
    assert!(workspace_proto_gen::generate_controllers(
        &out,
        &controllers,
        &out.join("controllers.rs")
    )
    .is_err());
    assert!(!controllers.join("root/a_new.rs").exists());
    assert_eq!(
        fs::read_to_string(out.join("controllers.rs")).unwrap(),
        "// existing registry\n"
    );
    assert_eq!(
        fs::read_to_string(controllers.join("root/z_bad.rs")).unwrap(),
        "pub struct ZBad; impl ZBad {"
    );
}
#[test]
fn removing_protocol_updates_registry_and_retains_business_source() {
    let temp = tempfile::tempdir().unwrap();
    let src = temp.path().join("src");
    let out = temp.path().join("out");
    let controllers = temp.path().join("controllers");
    protocol(&src, "root/keep__first.proto");
    protocol(&src, "root/removed__second.proto");
    generate(&src, &out, &controllers);
    let file = controllers.join("root/removed.rs");
    let original = fs::read(&file).unwrap();
    fs::remove_file(src.join("root/removed__second.proto")).unwrap();
    generate(&src, &out, &controllers);
    assert_eq!(original, fs::read(file).unwrap());
    let registry = fs::read_to_string(out.join("controllers.rs")).unwrap();
    assert!(!registry.contains("removed"));
    assert!(registry.contains("root_keep::Keep::first"));
}

#[test]
fn incrementally_added_methods_are_formatted_and_idempotent() {
    let temp = tempfile::tempdir().unwrap();
    let src = temp.path().join("src");
    let out = temp.path().join("out");
    let controllers = temp.path().join("controllers");
    protocol(&src, "root/test__first.proto");
    generate(&src, &out, &controllers);
    let before = fs::read_to_string(controllers.join("root/test.rs")).unwrap();
    protocol(&src, "root/test__second.proto");
    generate(&src, &out, &controllers);
    let file = controllers.join("root/test.rs");
    let changed = fs::read_to_string(&file).unwrap();
    assert!(changed.starts_with(before.trim_end_matches("}\n")));
    assert!(std::process::Command::new("rustfmt")
        .args(["--check", "--edition", "2021"])
        .arg(&file)
        .status()
        .unwrap()
        .success());
    workspace_proto_gen::generate_controllers(&out, &controllers, &out.join("controllers.rs"))
        .unwrap();
    assert_eq!(changed, fs::read_to_string(file).unwrap());
}
