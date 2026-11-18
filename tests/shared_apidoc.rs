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

fn referenced_docs() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    put(temp.path(), "common/api_options.proto", compiler::SCHEMA);
    put(
        temp.path(),
        "common/field_options.proto",
        field_model::SCHEMA,
    );
    put(
        temp.path(),
        "common/messages.proto",
        r#"syntax="proto3"; package common;
import "common/field_options.proto";
message Request {string id=1 [(common.field)={enum:"allowed"}];}
message Response {string result=1;}
message Docs {
  message Request {string query=1;}
  message Success {string result=1; Values.apidoc_value values=2;}
  message Failure {string reason=1;}
  message Result {oneof status {Success success=1; Failure failure=2;}}
  message Values {message apidoc_value {oneof value {string text=1; int32 count=2;}}}
}
"#,
    );
    put(
        temp.path(),
        "demo/test__get.proto",
        r#"syntax="proto3";
import "common/api_options.proto"; import "common/messages.proto";
option(common.api)={title:"引用消息" auth:PUBLIC input:"common.Request" output:"common.Response"
  apidoc_in:"common.Docs.Request" apidoc_out:"common.Docs.Result"
  examples:{request_json:'{"query":"hello"}' data_json:'{"status":"success","result":"ok","values":3}'}};
"#,
    );
    temp
}

#[test]
fn explicit_documentation_references_override_schemas_without_changing_runtime() {
    let temp = referenced_docs();
    for target in ["typescript", "php", "rust", "go"] {
        let w = compiler::compile(temp.path(), target).unwrap();
        let a = &w.endpoints[0];
        assert_eq!(a.runtime_api().input, "common.Request");
        assert_eq!(a.runtime_api().output, "common.Response");
        let project = serde_json::from_value(json!({"apifox_project_name":"test"})).unwrap();
        let doc = openapi::document(a, &w, &project).unwrap();
        let components = doc["components"]["schemas"].as_object().unwrap();
        let operation = &doc["paths"]["/demo/test/get"]["post"];
        let request = &operation["requestBody"]["content"]["application/json"]["schema"];
        assert!(request["properties"].get("query").is_some());
        assert!(request["properties"].get("id").is_none());
        let response = &operation["responses"]["200"]["content"]["application/json"]["schema"];
        let response_key = response["properties"]["data"]["$ref"]
            .as_str()
            .unwrap()
            .rsplit('/')
            .next()
            .unwrap();
        assert_eq!(response_key, "common.Docs.Result.response");
        assert_eq!(
            components[response_key]["oneOf"][0]["properties"]["status"]["const"],
            "success"
        );
        assert_eq!(
            components["common.Docs.Values.apidoc_value.response"]["oneOf"][1]["type"],
            "integer"
        );
        openapi::validate_instance(response, &json!({"code":0,"info":"ok","request_id":"r","data":{"status":"failure","reason":"missing"}}), components).unwrap();
        assert!(openapi::validate_instance(
            response,
            &json!({"code":0,"info":"ok","request_id":"r","data":{"status":"unknown"}}),
            components
        )
        .is_err());
        let map = workspace_proto_gen::fields::struct_map(&w, target == "php").unwrap();
        let prefix = if ["rust", "go"].contains(&target) {
            "project.demo.test__get"
        } else {
            "demo.test__get"
        };
        assert_eq!(
            map[&format!("{prefix}.apidoc_out")]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert!(w.rules["messages"]["common.Request"].is_object());
    }
}

#[test]
fn local_apidoc_out_supports_objects_branches_and_native_arrays() {
    for (definition, expected) in [
        ("message apidoc_out {string documented=1;}", "object"),
        ("message A {string a=1;} message B {string b=1;} message apidoc_out {oneof kind {A first=1; B second=2;}}", "oneOf"),
        ("message apidoc_out {message apidoc_value {repeated string value=1;} apidoc_value items=1;}", "array"),
    ] {
        let temp = referenced_docs();
        put(temp.path(), "demo/test__get.proto", &format!(r#"syntax="proto3";
import "common/api_options.proto";
option(common.api)={{title:"本地文档" auth:PUBLIC}};
message in {{}} message out {{string actual=1;}}
{definition}"#));
        let w = compiler::compile(temp.path(), "typescript").unwrap();
        let project = serde_json::from_value(json!({"apifox_project_name":"test"})).unwrap();
        let doc = openapi::document(&w.endpoints[0], &w, &project).unwrap();
        let schemas = &doc["components"]["schemas"];
        let schema = &schemas["proto.project.demo.test__get.apidoc_out.response"];
        match expected {
            "object" => assert!(schema["properties"].get("documented").is_some()),
            "oneOf" => assert_eq!(schema["oneOf"][0]["properties"]["kind"]["const"], "first"),
            _ => assert_eq!(schemas["proto.project.demo.test__get.apidoc_out.apidoc_value.response"]["type"], "array"),
        }
        assert_eq!(w.endpoints[0].runtime_api().output, "proto.project.demo.test__get.out");
    }
}

#[test]
fn message_references_fail_with_actionable_errors() {
    for option in ["input", "output", "apidoc_in", "apidoc_out"] {
        for failure in ["missing", "import", "conflict"] {
            let temp = referenced_docs();
            let path = temp.path().join("demo/test__get.proto");
            let text = fs::read_to_string(&path).unwrap();
            let value = match option {
                "input" => "common.Request",
                "output" => "common.Response",
                "apidoc_in" => "common.Docs.Request",
                _ => "common.Docs.Result",
            };
            let local = match option {
                "input" => "in",
                "output" => "out",
                x => x,
            };
            let text = match failure {
                "missing" => text.replace(value, "common.Missing"),
                "import" => {
                    // Isolate each option so its own diagnostic is reached.
                    format!("syntax=\"proto3\";import \"common/api_options.proto\";option(common.api)={{title:\"x\" auth:PUBLIC {option}:\"{value}\"}}; message in{{}}message out{{}}")
                }
                _ => text + &format!("\nmessage {local}{{}}\n"),
            };
            fs::write(path, text).unwrap();
            let error = compiler::compile(temp.path(), "typescript")
                .err()
                .unwrap()
                .to_string();
            assert!(error.contains(option), "{error}");
            if failure == "import" {
                assert!(
                    error.contains("import \"common/messages.proto\";"),
                    "{error}"
                );
            }
        }
    }
}

#[test]
fn referenced_runtime_fallback_get_parameters_and_raw_response_docs() {
    let temp = referenced_docs();
    let protocol = temp.path().join("demo/test__get.proto");
    put(
        temp.path(),
        "demo/test__get.proto",
        r#"syntax="proto3";
import "common/api_options.proto"; import "common/messages.proto";
option(common.api)={title:"回退" auth:PUBLIC input:"common.Request" output:"common.Response" http:{method:GET}};
"#,
    );
    let project = serde_json::from_value(json!({"apifox_project_name":"test"})).unwrap();
    let w = compiler::compile(temp.path(), "typescript").unwrap();
    let a = &w.endpoints[0];
    assert_eq!(
        a.documentation_message(&w.pool, false).full_name(),
        "common.Request"
    );
    assert_eq!(
        a.documentation_message(&w.pool, true).full_name(),
        "common.Response"
    );
    let doc = openapi::document(a, &w, &project).unwrap();
    assert_eq!(
        doc["paths"]["/demo/test/get"]["get"]["parameters"][0]["name"],
        "id"
    );
    let text = fs::read_to_string(&protocol).unwrap().replace("http:{method:GET}", "http:{method:GET} apidoc_in:\"common.Docs.Request\" apidoc_out:\"common.Docs.Result\" docs:{response_mode:RAW_JSON}");
    fs::write(protocol, text).unwrap();
    let w = compiler::compile(temp.path(), "typescript").unwrap();
    let doc = openapi::document(&w.endpoints[0], &w, &project).unwrap();
    let operation = &doc["paths"]["/demo/test/get"]["get"];
    assert_eq!(operation["parameters"][0]["name"], "query");
    let response = &operation["responses"]["200"]["content"]["application/json"]["schema"];
    assert_eq!(
        response["$ref"],
        "#/components/schemas/common.Docs.Result.response"
    );
    assert!(response.get("properties").is_none());
}
