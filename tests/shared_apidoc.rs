use serde_json::{json, Value};
use std::{fs, path::Path};
use workspace_proto_gen::{compiler, field_model, openapi};

fn put(root: &Path, name: &str, source: &str) {
    let path = root.join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, source).unwrap();
}

#[test]
fn imported_discriminated_inputs_are_shared_by_single_requests_and_batch_arrays() {
    let temp = tempfile::tempdir().unwrap();
    put(temp.path(), "common/api_options.proto", compiler::SCHEMA);
    put(
        temp.path(),
        "common/field_options.proto",
        field_model::SCHEMA,
    );
    put(
        temp.path(),
        "common/task_docs.proto",
        r#"syntax="proto3"; package common;
import "common/field_options.proto";
message TaskDocs {
  message Chat {string messages=1;}
  message Decision {string state=1;}
  message apidoc_in {oneof task_type {Chat chat=1; Decision decision=2;}}
  message List {message apidoc_value {repeated apidoc_in value=1 [(common.field)={min_items:1 max_items:100}];}}
}"#,
    );
    put(
        temp.path(),
        "llm/task__create.proto",
        r#"syntax="proto3";
import "common/api_options.proto"; import "common/task_docs.proto";
option(common.api)={title:"单任务" auth:PUBLIC};
message in {optional string task_type=1;}
message apidoc_in {oneof task_type {common.TaskDocs.Chat chat=1; common.TaskDocs.Decision decision=2;}}
message out {}"#,
    );
    put(
        temp.path(),
        "llm/task__create_batch.proto",
        r#"syntax="proto3";
import "common/api_options.proto"; import "common/task_docs.proto";
option(common.api)={title:"批次" auth:PUBLIC};
message in {string client_batch_id=1; repeated string tasks=2;}
message apidoc_in {string client_batch_id=1; common.TaskDocs.List.apidoc_value tasks=2;}
message out {}"#,
    );
    let workspace = compiler::compile(temp.path(), "typescript").unwrap();
    let project = serde_json::from_value(json!({"apifox_project_name":"test"})).unwrap();
    let documents: Vec<Value> = workspace
        .endpoints
        .iter()
        .map(|endpoint| openapi::document(endpoint, &workspace, &project).unwrap())
        .collect();
    let single = documents
        .iter()
        .find(|doc| doc["paths"].get("/llm/task/create").is_some())
        .unwrap();
    let batch = documents
        .iter()
        .find(|doc| doc["paths"].get("/llm/task/create_batch").is_some())
        .unwrap();
    let schema = &batch["paths"]["/llm/task/create_batch"]["post"]["requestBody"]["content"]
        ["application/json"]["schema"];
    assert_eq!(schema["required"], json!(["client_batch_id", "tasks"]));
    let components = batch["components"]["schemas"].as_object().unwrap();
    let array = &components["common.TaskDocs.List.apidoc_value.request"];
    assert_eq!(array["type"], "array");
    assert_eq!(array["minItems"], 1);
    assert_eq!(array["maxItems"], 100);
    let branches = &components["common.TaskDocs.apidoc_in.request"]["oneOf"];
    assert_eq!(
        branches,
        &single["paths"]["/llm/task/create"]["post"]["requestBody"]["content"]["application/json"]
            ["schema"]["oneOf"]
    );
    let good = json!({"client_batch_id":"batch", "tasks":[{"task_type":"chat","messages":"hello"},{"task_type":"decision","state":"context"}]});
    openapi::validate_instance(schema, &good, components).unwrap();
    for tasks in [
        json!([]),
        json!([{"task_type":"decision"}]),
        json!([{"task_type":"other"}]),
        json!(vec![json!({"task_type":"chat","messages":"hello"}); 101]),
    ] {
        assert!(openapi::validate_instance(
            schema,
            &json!({"client_batch_id":"batch","tasks":tasks}),
            components
        )
        .is_err());
    }
}
