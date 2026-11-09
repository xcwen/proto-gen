use std::fs;
use workspace_proto_gen::{compiler, navigation, project};

#[test]
fn navigation_matches_all_target_names_and_real_routes() {
    for (target, file, owner, symbol) in [
        ("php", "AdminApi/AiTask.php", "AiTask", "user_list"),
        ("typescript", "admin_api/ai_task.ts", "ai_task", "user_list"),
        ("rust", "admin_api/ai_task.rs", "AiTask", "user_list"),
        ("go", "admin_api/ai_task.go", "AiTask", "UserList"),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let proto = temp.path().join("proto");
        fs::create_dir_all(proto.join("src/admin_api")).unwrap();
        fs::create_dir_all(proto.join("src/common")).unwrap();
        fs::write(proto.join("src/common/api_options.proto"), compiler::SCHEMA).unwrap();
        fs::write(proto.join("src/admin_api/ai_task__user_list.proto"), r#"syntax="proto3";
import "common/api_options.proto";
option(common.api)={title:"List" auth:PUBLIC tags:"_uri_no_project" http:{method:GET path:"/custom/list"}};
message in{} message out{}"#).unwrap();
        fs::write(proto.join("generator.toml"), format!("version=1\ntarget=\"{target}\"\nsource=\"src\"\noutput=\"../gen\"\ncontrollers=\"../controllers\"\n")).unwrap();
        let cfg = compiler::Config::load(&proto.join("generator.toml")).unwrap();
        let work = compiler::compile(&cfg.source, target).unwrap();
        let index = navigation::index(&cfg, &work).unwrap();
        let e = &index.endpoints[0];
        assert_eq!(index.version, 1);
        assert_eq!(e.id, "admin_api/ai_task/user_list");
        assert_eq!(e.method, "GET");
        assert_eq!(e.proto, "proto/src/admin_api/ai_task__user_list.proto");
        assert_eq!(e.controller.file, format!("controllers/{file}"));
        assert_eq!(e.controller.owner, owner);
        assert_eq!(e.controller.symbol, symbol);
        assert_eq!(
            e.paths,
            if target == "php" {
                vec!["/custom/list", "/ai_task/user_list"]
            } else {
                vec!["/custom/list"]
            }
        );
    }
}

#[test]
fn navigation_is_managed_and_check_never_repairs_it() {
    let temp = tempfile::tempdir().unwrap();
    let proto = temp.path().join("proto");
    fs::create_dir_all(proto.join("src/root")).unwrap();
    fs::create_dir_all(proto.join("src/common")).unwrap();
    fs::write(proto.join("src/common/api_options.proto"), compiler::SCHEMA).unwrap();
    fs::write(proto.join("src/root/test__sum.proto"), "syntax=\"proto3\";import \"common/api_options.proto\";option(common.api)={title:\"Sum\" auth:PUBLIC};message in{}message out{}").unwrap();
    let config = proto.join("generator.toml");
    fs::write(&config, "version=1\ntarget=\"php\"\nsource=\"src\"\noutput=\"../gen\"\ncontrollers=\"../controllers\"\n").unwrap();
    project::run(&config, false).unwrap();
    let path = proto.join("navigation.json");
    let original = fs::read(&path).unwrap();
    let json: serde_json::Value = serde_json::from_slice(&original).unwrap();
    assert_eq!(json["endpoints"][0]["method"], "POST");
    assert!(fs::read_to_string(proto.join(".proto-gen-manifest.json"))
        .unwrap()
        .contains("proto/navigation.json"));
    fs::write(&path, "stale").unwrap();
    assert!(project::run(&config, true).is_err());
    assert_eq!(fs::read_to_string(&path).unwrap(), "stale");
    project::run(&config, false).unwrap();
    assert_eq!(fs::read(&path).unwrap(), original);
    fs::remove_file(&path).unwrap();
    project::run(&config, false).unwrap();
    assert_eq!(fs::read(&path).unwrap(), original);
    project::run(&config, true).unwrap();
}
