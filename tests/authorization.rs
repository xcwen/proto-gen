use serde_json::{json, Value};
use std::{fs, path::Path};
use workspace_proto_gen::{
    compiler::{self, Config},
    project,
};

fn put(root: &Path, relative: &str, text: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}
fn config() -> Value {
    json!({"authorization":{
        "schema_version":1,
        "enforced_route_prefixes":["/llm/"],
        "enforced_source_groups":["llm"],
        "projects":{"llm":{"tenant_id":126,"project_type":"llm"}},
        "policies":{"provider_llm":{
            "kind":"provider_token","project_keys":["llm"],
            "allow_account_binding":false,"allow_project_sub_token":true,
            "api_types":{"mode":"not_applicable"}
        }}
    }})
}
fn protocol(metadata: &str) -> String {
    format!("syntax=\"proto3\"; import \"common/api_options.proto\"; option(common.api)={{title:\"Account\" auth:SESSION {metadata}}}; message in{{}} message out{{string value=1;}}")
}
fn fixture(target: &str) -> (tempfile::TempDir, Config) {
    let dir = tempfile::tempdir().unwrap();
    put(
        dir.path(),
        "proto/src/common/api_options.proto",
        compiler::SCHEMA,
    );
    put(dir.path(), "proto/config.json", &config().to_string());
    put(
        dir.path(),
        "proto/src/llm/account__info.proto",
        &protocol("authorization:{policy:\"provider_llm\"}"),
    );
    put(
        dir.path(),
        "proto/src/llm_admin/manage__submit.proto",
        &protocol(""),
    );
    let registry = if target == "go" {
        "registry.go"
    } else {
        "registry.rs"
    };
    put(dir.path(), "proto/generator.toml", &format!("version=1\ntarget=\"{target}\"\nsource=\"src\"\noutput=\"../gen/proto\"\ncontrollers=\"../controllers\"\nregistry=\"../gen/{registry}\"\n"));
    let cfg = Config::load(&dir.path().join("proto/generator.toml")).unwrap();
    (dir, cfg)
}
fn write_config(dir: &Path, value: &Value) {
    put(dir, "proto/config.json", &value.to_string());
}

#[test]
fn same_policy_and_coverage_for_every_target_with_legacy_routes_untouched() {
    let mut baseline = None;
    for target in ["typescript", "php", "rust", "go"] {
        let (_dir, cfg) = fixture(target);
        let work = compiler::compile(&cfg.source, target).unwrap();
        let bundle = work.authorization.unwrap();
        assert_eq!(bundle.routes.len(), 1);
        assert_eq!(bundle.routes[0].route_id, "llm.account__info");
        assert_eq!(
            bundle.routes[0].authorization.required_capabilities,
            vec!["project_sub_token.v1", "provider_token.v1"]
        );
        assert!(work
            .endpoints
            .iter()
            .find(|a| a.group == "llm_admin")
            .unwrap()
            .authorization
            .is_none());
        if let Some(expected) = &baseline {
            assert_eq!(expected, &bundle);
        } else {
            baseline = Some(bundle);
        }
    }
}

#[test]
fn missing_policy_is_rejected_for_new_routes_and_custom_http_paths() {
    for metadata in ["", "http:{path:\"/custom/account\"}"] {
        let (dir, cfg) = fixture("typescript");
        put(
            dir.path(),
            "proto/src/llm/task__create.proto",
            &protocol(metadata),
        );
        let error = compiler::compile(&cfg.source, "typescript")
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("missing authorization policy"), "{error}");
    }
    let (dir, cfg) = fixture("typescript");
    put(
        dir.path(),
        "proto/src/llm/account__info.proto",
        &protocol("authorization:{policy:\"provider_llm\"} http:{path:\"/custom/account\"}"),
    );
    let bundle = compiler::compile(&cfg.source, "typescript")
        .unwrap()
        .authorization
        .unwrap();
    assert_eq!(bundle.routes[0].canonical_path, "/custom/account");
}

#[test]
fn canonical_prefix_and_explicit_outside_scope_references_are_protected() {
    let (dir, cfg) = fixture("typescript");
    put(
        dir.path(),
        "proto/src/other/account__info.proto",
        &protocol("http:{path:\"/llm/other/info\"}"),
    );
    assert!(compiler::compile(&cfg.source, "typescript").is_err());
    put(
        dir.path(),
        "proto/src/other/account__info.proto",
        &protocol("authorization:{policy:\"provider_llm\"} http:{path:\"/custom/other/info\"}"),
    );
    let bundle = compiler::compile(&cfg.source, "typescript")
        .unwrap()
        .authorization
        .unwrap();
    assert_eq!(bundle.routes.len(), 2);
    assert!(bundle
        .routes
        .iter()
        .any(|r| r.canonical_path == "/custom/other/info"));
}

#[test]
fn invalid_configuration_and_references_fail_closed() {
    type Mutation = Box<dyn Fn(&mut Value)>;
    let mutations: Vec<Mutation> = vec![
        Box::new(|v| v["authorization"]["schema_version"] = json!(2)),
        Box::new(|v| v["authorization"]["policies"]["provider_llm"]["kind"] = json!("unknown")),
        Box::new(|v| v["authorization"]["policies"]["provider_llm"]["project_keys"] = json!([])),
        Box::new(|v| {
            v["authorization"]["policies"]["provider_llm"]["project_keys"] = json!(["unknown"])
        }),
        Box::new(|v| {
            v["authorization"]["policies"]["provider_llm"]["allow_account_binding"] = Value::Null
        }),
        Box::new(|v| v["authorization"]["policies"]["provider_llm"]["ignored"] = json!(true)),
        Box::new(|v| v["authorization"]["enforced_route_prefixes"] = json!(["/llm"])),
        Box::new(|v| v["authorization"]["enforced_source_groups"] = json!(["llm/"])),
        Box::new(|v| {
            v["authorization"]["projects"]["duplicate"] =
                json!({"tenant_id":126,"project_type":"llm"})
        }),
    ];
    for mutate in mutations {
        let (dir, cfg) = fixture("typescript");
        let mut value = config();
        mutate(&mut value);
        write_config(dir.path(), &value);
        assert!(
            compiler::compile(&cfg.source, "typescript").is_err(),
            "accepted {value}"
        );
    }
    for metadata in [
        "authorization:{}",
        "authorization:{policy:\"unknown\"}",
        "authorization:{policy:\"provider_llm\"}",
    ] {
        let (dir, cfg) = fixture("typescript");
        let proto = protocol(metadata);
        let proto = if metadata.contains("provider_llm") {
            proto.replace("auth:SESSION", "auth:PUBLIC")
        } else {
            proto
        };
        put(dir.path(), "proto/src/llm/account__info.proto", &proto);
        assert!(compiler::compile(&cfg.source, "typescript").is_err());
    }
}

#[test]
fn unconfigured_projects_keep_the_previous_runtime_contract() {
    let (dir, cfg) = fixture("rust");
    fs::remove_file(dir.path().join("proto/config.json")).unwrap();
    put(
        dir.path(),
        "proto/src/llm/account__info.proto",
        &protocol(""),
    );
    let work = compiler::compile(&cfg.source, "rust").unwrap();
    assert!(work.authorization.is_none());
    assert!(work.endpoints.iter().all(|a| a.authorization.is_none()));
    let api = serde_json::to_value(work.endpoints[0].runtime_api()).unwrap();
    assert!(api.get("authorization").is_none());
    assert!(work.endpoints[0].manifest().get("AUTHORIZATION").is_none());
    project::run(&dir.path().join("proto/generator.toml"), false).unwrap();
    assert!(!cfg.output.join("route_policy.json").exists());
    assert!(!fs::read_to_string(cfg.output.join("api.rs"))
        .unwrap()
        .contains("AUTHORIZATION_JSON"));
    put(
        dir.path(),
        "proto/src/llm/account__info.proto",
        &protocol("authorization:{policy:\"provider_llm\"}"),
    );
    assert!(compiler::compile(&cfg.source, "rust").is_err());
}

#[test]
fn policy_changes_invalidate_generation_but_documentation_does_not_change_hashes() {
    let (dir, cfg) = fixture("php");
    let config_path = dir.path().join("proto/generator.toml");
    project::run(&config_path, false).unwrap();
    let before = fs::read(cfg.output.join("route_policy.json")).unwrap();
    let mut value = config();
    value["authorization"]["policies"]["provider_llm"]["allow_project_sub_token"] = json!(false);
    write_config(dir.path(), &value);
    assert!(project::run(&config_path, true).is_err());
    project::run(&config_path, false).unwrap();
    let changed = fs::read(cfg.output.join("route_policy.json")).unwrap();
    assert_ne!(before, changed);
    put(
        dir.path(),
        "proto/src/llm/account__info.proto",
        &protocol(
            "authorization:{policy:\"provider_llm\"} description:\"Changed docs\" tags:\"extra\"",
        ),
    );
    project::run(&config_path, false).unwrap();
    assert_eq!(
        changed,
        fs::read(cfg.output.join("route_policy.json")).unwrap()
    );
    project::run(&config_path, true).unwrap();
}

#[test]
fn api_type_symbols_use_the_shared_catalog_and_unknown_values_are_rejected() {
    let (dir, cfg) = fixture("php");
    let mut value = config();
    value["authorization"]["policies"]["provider_llm"]["api_types"] =
        json!({"mode":"any_of","values":["llm"],"empty_list":"allow"});
    write_config(dir.path(), &value);
    put(dir.path(),"proto/enum_catalog.json",&json!({"version":1,"enums":{"api_type":{"field_list":[{"field_name":"llm","field_value":3}]}}}).to_string());
    let work = compiler::compile(&cfg.source, "php").unwrap();
    let authorization = serde_json::to_value(
        work.endpoints
            .iter()
            .find(|a| a.group == "llm")
            .unwrap()
            .authorization
            .as_ref()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        authorization["resolved_policy"]["api_types"]["values"],
        json!([3])
    );
    assert!(authorization["required_capabilities"]
        .as_array()
        .unwrap()
        .contains(&json!("api_types.v1")));
    value["authorization"]["policies"]["provider_llm"]["api_types"]["values"] = json!(["unknown"]);
    write_config(dir.path(), &value);
    assert!(compiler::compile(&cfg.source, "php").is_err());
}

#[test]
fn target_outputs_carry_policy_into_actual_route_registration() {
    let mut baseline = None;
    for target in ["php", "rust", "typescript", "go"] {
        let (dir, cfg) = fixture(target);
        if target == "typescript" {
            let Some(modules) = std::env::var_os("PROTO_GEN_TEST_TS_NODE_MODULES") else {
                continue;
            };
            #[cfg(unix)]
            std::os::unix::fs::symlink(modules, dir.path().join("node_modules")).unwrap();
        }
        if target == "go" {
            let Some(plugin) = std::env::var_os("PROTO_GEN_TEST_GO_PLUGIN") else {
                continue;
            };
            fs::create_dir_all(dir.path().join(".tools")).unwrap();
            fs::copy(plugin, dir.path().join(".tools/protoc-gen-go")).unwrap();
            put(dir.path(), "go.mod", "module test-core\n\ngo 1.24.4\n");
        }
        project::run(&dir.path().join("proto/generator.toml"), false).unwrap();
        let bundle: Value =
            serde_json::from_slice(&fs::read(cfg.output.join("route_policy.json")).unwrap())
                .unwrap();
        if let Some(expected) = &baseline {
            assert_eq!(expected, &bundle)
        } else {
            baseline = Some(bundle)
        };
        match target {
            "php" => {
                let routes = fs::read_to_string(cfg.output.join("Proto/cmd_list.php")).unwrap();
                assert!(routes.contains("'authorization' => array"));
                assert_eq!(routes.matches("'authorization'").count(), 1);
            }
            "rust" => {
                let api: Value =
                    serde_json::from_slice(&fs::read(cfg.output.join("api.json")).unwrap())
                        .unwrap();
                assert_eq!(
                    api.as_array()
                        .unwrap()
                        .iter()
                        .filter(|r| r.get("authorization").is_some())
                        .count(),
                    1
                );
                assert!(fs::read_to_string(cfg.output.join("api.rs"))
                    .unwrap()
                    .contains("const AUTHORIZATION_JSON: Option<&'static str> = Some("));
            }
            "typescript" => {
                let routes = fs::read_to_string(cfg.output.join("cmd_map.ts")).unwrap();
                assert!(routes.contains("authorization?: RouteAuthorization"));
                assert_eq!(routes.matches("        authorization:").count(), 1);
                assert!(cfg.output.join("route_policy.ts").exists());
            }
            "go" => {
                let routes = fs::read_to_string(cfg.registry.as_ref().unwrap()).unwrap();
                assert!(routes.contains("Authorization: []byte("));
                assert_eq!(routes.matches("Authorization:").count(), 1);
            }
            _ => unreachable!(),
        }
        project::run(&dir.path().join("proto/generator.toml"), true).unwrap();
    }
}
