use serde_json::{json, Value};
use std::{fs, path::Path};
use workspace_proto_gen::{compiler, field_model, openapi};

fn put(root: &Path, name: &str, source: &str) {
    let path = root.join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, source).unwrap();
}

fn fixture(union: &str) -> (tempfile::TempDir, compiler::Workspace) {
    let temp = tempfile::tempdir().unwrap();
    put(temp.path(), "common/api_options.proto", compiler::SCHEMA);
    put(
        temp.path(),
        "common/field_options.proto",
        field_model::SCHEMA,
    );
    put(
        temp.path(),
        "demo/task__create.proto",
        &format!(
            r#"syntax="proto3";
import "common/api_options.proto";
import "common/field_options.proto";
import "google/protobuf/struct.proto";
option(common.api)={{title:"任务" auth:PUBLIC}};
message in {{ optional string task_type=1; google.protobuf.Value state=2; }}
message decision_request {{ apidoc_value state=1; Ordinary selection=2; }}
message Ordinary {{ oneof selection {{string label=1; int32 count=2;}} }}
message apidoc_value {{ {union} }}
message apidoc_in {{ oneof task_type {{ decision_request decision=1; }} }}
message out {{}}
"#
        ),
    );
    let workspace = compiler::compile(temp.path(), "typescript").unwrap();
    (temp, workspace)
}

fn document(workspace: &compiler::Workspace) -> anyhow::Result<Value> {
    let project = serde_json::from_value(json!({"apifox_project_name":"test"})).unwrap();
    openapi::document(&workspace.endpoints[0], workspace, &project)
}

const UNION: &str = r#"oneof value {
  string text=1 [(common.field)={min_length:1 examples:"客户申请退款"}]; // 文本
  google.protobuf.Struct object=2 [(common.field)={examples:'{"text":"退款"}'}]; // JSON对象
  google.protobuf.ListValue array=3 [(common.field)={examples:'["客户消息","历史记录"]'}]; // JSON数组
}"#;

#[test]
fn native_json_union_has_named_choices_and_keeps_the_real_input_and_normal_oneofs() {
    let (_temp, workspace) = fixture(UNION);
    let doc = document(&workspace).unwrap();
    let components = doc["components"]["schemas"].as_object().unwrap();
    let key = "proto.project.demo.task__create.apidoc_value.request";
    let choices = components[key]["oneOf"].as_array().unwrap();
    assert_eq!(
        choices
            .iter()
            .map(|s| s["type"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["string", "object", "array"]
    );
    assert_eq!(
        choices
            .iter()
            .map(|s| s["title"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["文本", "JSON对象", "JSON数组"]
    );
    assert_eq!(choices[0]["examples"], json!(["客户申请退款"]));
    assert_eq!(choices[0]["minLength"], 1);
    assert_eq!(choices[1]["examples"], json!([{"text":"退款"}]));
    assert_eq!(choices[2]["examples"], json!([["客户消息", "历史记录"]]));
    assert!(components[key].get("properties").is_none());

    for value in [
        json!("客户申请退款"),
        json!({"text":"退款"}),
        json!(["消息"]),
    ] {
        openapi::validate_instance(&components[key], &value, components).unwrap();
    }
    for value in [json!(""), json!(null), json!(true), json!(7)] {
        assert!(openapi::validate_instance(&components[key], &value, components).is_err());
    }
    let normal = &components["proto.project.demo.task__create.Ordinary.request"];
    assert_eq!(normal["type"], "object");
    assert!(normal["properties"].get("label").is_some());
    assert!(normal["allOf"][0]["oneOf"].is_array());
    let state = workspace
        .pool
        .get_message_by_name("proto.project.demo.task__create.in")
        .unwrap()
        .get_field_by_name("state")
        .unwrap();
    assert!(
        matches!(state.kind(), prost_reflect::Kind::Message(m) if m.full_name() == "google.protobuf.Value")
    );
}

#[test]
fn malformed_documentation_value_unions_are_rejected() {
    for union in [
        "string text=1;",
        "oneof kind {string text=1;}",
        "oneof value {string text=1;} string extra=2;",
    ] {
        let (_temp, workspace) = fixture(union);
        assert!(document(&workspace)
            .unwrap_err()
            .to_string()
            .contains("apidoc_value needs exactly one oneof value"));
    }
}

#[test]
fn documentation_value_constraints_do_not_validate_wrapped_fields_at_runtime() {
    let (_temp, workspace) =
        fixture(r#"oneof value {string text=1 [(common.field)={enum:"allowed"}];}"#);
    document(&workspace).unwrap();
    let messages = workspace.rules["messages"].as_object().unwrap();
    assert!(!messages.contains_key("proto.project.demo.task__create.apidoc_value"));
    assert!(!messages.contains_key("proto.project.demo.task__create.decision_request"));
}

#[test]
fn native_map_and_array_wrappers_keep_collection_constraints_and_required_presence() {
    for (source, expected_type, good, bad) in [
        (
            r#"map<string,string> value=1 [(common.field)={min_items:1 max_items:2 items:{min_length:1}}];"#,
            "object",
            json!({"refund":"申请退款"}),
            vec![
                json!({}),
                json!({"a":"1","b":"2","c":"3"}),
                json!({"refund":""}),
                json!([]),
            ],
        ),
        (
            r#"repeated string value=1 [(common.field)={min_items:2 max_items:3 items:{min_length:1}}];"#,
            "array",
            json!(["低", "中", "高"]),
            vec![
                json!([]),
                json!(["低"]),
                json!(["低", "中", "高", "极高"]),
                json!(["低", ""]),
                json!({}),
            ],
        ),
    ] {
        let (_temp, workspace) = fixture(source);
        let doc = document(&workspace).unwrap();
        let components = doc["components"]["schemas"].as_object().unwrap();
        let schema = &components["proto.project.demo.task__create.apidoc_value.request"];
        assert_eq!(schema["type"], expected_type);
        assert!(schema.get("properties").is_none());
        if expected_type == "object" {
            assert_eq!(schema["minProperties"], 1);
            assert_eq!(schema["maxProperties"], 2);
            assert!(schema.get("minItems").is_none());
        }
        openapi::validate_instance(schema, &good, components).unwrap();
        for value in bad {
            assert!(openapi::validate_instance(schema, &value, components).is_err());
        }
        let decision = &doc["paths"]["/demo/task/create"]["post"]["requestBody"]["content"]
            ["application/json"]["schema"]["oneOf"][0];
        assert!(decision["required"]
            .as_array()
            .unwrap()
            .contains(&json!("state")));
    }
}

#[test]
fn native_map_examples_must_match_map_size_limits() {
    let source = r#"map<string,string> value=1 [(common.field)={min_items:1 examples:"{}"}];"#;
    let temp = tempfile::tempdir().unwrap();
    put(temp.path(), "common/api_options.proto", compiler::SCHEMA);
    put(
        temp.path(),
        "common/field_options.proto",
        field_model::SCHEMA,
    );
    put(
        temp.path(),
        "demo/job__create.proto",
        &format!(
            r#"syntax="proto3";
import "common/api_options.proto";import "common/field_options.proto";
option(common.api)={{title:"任务" auth:PUBLIC}};
message in {{ {source} }} message out {{}}"#
        ),
    );
    assert!(compiler::compile(temp.path(), "typescript").is_err());
}
