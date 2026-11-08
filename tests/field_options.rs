use serde_json::{json, Value};
use std::{fs, path::Path};
use workspace_proto_gen::{compiler, openapi};
fn put(root: &Path, path: &str, text: &str) {
    let p = root.join(path);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, text).unwrap();
}
fn fixture(fields: &str, options: &str) -> (tempfile::TempDir, compiler::Workspace) {
    let temp = tempfile::tempdir().unwrap();
    put(
        temp.path(),
        "proto/src/common/api_options.proto",
        compiler::SCHEMA,
    );
    put(
        temp.path(),
        "proto/src/common/field_options.proto",
        workspace_proto_gen::field_model::SCHEMA,
    );
    put(
        temp.path(),
        "proto/src/common/doc_fields.proto",
        r#"syntax="proto3";package common;import "common/field_options.proto";
option(common.doc_fields)={templates:{name:"common.label" field:{description:"公共说明" default:"base" examples:"shared"}}};"#,
    );
    put(
        temp.path(),
        "proto/enum_catalog.json",
        r#"{"version":1,"enums":{"boolean":{"field_list":[{"field_value":0,"field_name":"FALSE","field_desc":"否"},{"field_value":1,"field_name":"TRUE","field_desc":"是"}]}}}"#,
    );
    put(
        temp.path(),
        "proto/src/demo/job__list.proto",
        &format!(
            r#"syntax="proto3";
import "common/api_options.proto";import "common/field_options.proto";
option(common.api)={{title:"列表" auth:SESSION category_path:"小程序" category_path:"公共" tags:"demo" maintainers:"ryan" maintainers:"jim" {options}}};
message in {{{fields}}}
message out {{uint64 total=1; repeated Item list=2;}}
message Item {{optional string title=1; Item next=2;}}
"#
        ),
    );
    let w = compiler::compile(&temp.path().join("proto/src"), "rust").unwrap();
    (temp, w)
}
fn doc(w: &compiler::Workspace) -> Value {
    let p = serde_json::from_value(json!({"apifox_project_name":"test"})).unwrap();
    openapi::document(&w.endpoints[0], w, &p).unwrap()
}
#[test]
fn optional_default_container_references_and_response_examples() {
    let (_t, w) = fixture(
        r#"
uint32 required_default=1 [(common.field)={default:"0"}];
optional uint32 limit=2 [(common.field)={default:"10" max_uint:500}];
repeated string ids=3 [(common.field)={default:"[]" examples:'["a"]' items:{min_length:1}}];
optional string title=4 [(common.field)={template_ref:"common.label" clear:CLEAR_EXAMPLES default:""}];
optional string description=5 [(common.field)={description_ref:"common.label"}];
optional bool flag=6 [(common.field)={default:"false"}];
optional string nullable=7 [(common.field)={nullable:true default_null:true}];
map<string,Item> map=8;
"#,
        r#"deprecated:true deprecation_message:"请使用新版" examples:{name:"ok" data_json:'{"total":"18446744073709551615","list":[]}' }"#,
    );
    assert!(w.pool.get_message_by_name("common.DocFields").is_none());
    let d = doc(&w);
    let op = &d["paths"]["/demo/job/list"]["post"];
    let s = &op["requestBody"]["content"]["application/json"]["schema"];
    assert_eq!(s["required"], json!(["required_default"]));
    assert_eq!(s["properties"]["ids"]["default"], json!([]));
    assert_eq!(s["properties"]["ids"]["examples"], json!([["a"]]));
    assert!(s["properties"]["ids"]["items"].get("default").is_none());
    assert_eq!(s["properties"]["title"]["default"], "");
    assert!(s["properties"]["title"].get("examples").is_none());
    assert_eq!(s["properties"]["description"]["description"], "公共说明");
    assert!(s["properties"]["description"].get("default").is_none());
    assert_eq!(
        s["properties"]["nullable"]["type"],
        json!(["string", "null"])
    );
    assert_eq!(op["requestBody"]["required"], true);
    assert_eq!(op["x-apifox-folder"], "小程序/公共");
    assert_eq!(op["x-apifox-maintainer"], "ryan");
    assert_eq!(op["deprecated"], true);
    assert!(op["description"].as_str().unwrap().contains("请使用新版"));
    assert_eq!(
        op["responses"]["200"]["content"]["application/json"]["examples"]["ok"]["value"]["data"]
            ["total"],
        "18446744073709551615"
    );
    let c = d["components"]["schemas"].as_object().unwrap();
    assert!(openapi::validate_instance(s, &json!({"required_default":0,"limit":501}), c).is_err());
    assert!(openapi::validate_instance(s, &json!({"required_default":0,"ids":[""]}), c).is_err());
    assert!(openapi::validate_instance(s, &json!({"required_default":0,"flag":false}), c).is_ok());
    assert!(openapi::validate_instance(s, &json!({"required_default":0,"flag":null}), c).is_err());
}
#[test]
fn enum_number_and_exact_field_name_and_items_rules() {
    let (_t, w) = fixture(
        r#"optional uint32 flag=1 [(common.field)={enum_number:"boolean" default:"0"}];
string name=2 [(common.field)={enum_str:"boolean"}];
repeated string names=3 [(common.field)={items:{enum_str:"boolean"}}];
optional uint64 large=4 [(common.field)={max_uint:18446744073709551614 default:"18446744073709551614"}];"#,
        "",
    );
    let d = doc(&w);
    let s = &d["paths"]["/demo/job/list"]["post"]["requestBody"]["content"]["application/json"]
        ["schema"];
    let c = d["components"]["schemas"].as_object().unwrap();
    assert_eq!(s["properties"]["flag"]["enum"], json!([0, 1]));
    assert_eq!(s["properties"]["name"]["enum"], json!(["FALSE", "TRUE"]));
    assert!(
        openapi::validate_instance(s, &json!({"name":"TRUE","flag":0,"names":["FALSE"]}), c)
            .is_ok()
    );
    for v in [
        json!({"name":"true"}),
        json!({"name":"TRUE","flag":2}),
        json!({"name":"TRUE","names":["false"]}),
        json!({"name":"TRUE","large":"18446744073709551615"}),
    ] {
        assert!(openapi::validate_instance(s, &v, c).is_err());
    }
    assert_eq!(
        w.rules["messages"]["proto.project.demo.job__list.in"]["names"]["kind"],
        "literal"
    );
}
#[test]
fn invalid_field_metadata_is_rejected_before_generation() {
    for fields in [
        r#"uint32 flag=1 [(common.field)={enum_number:"missing"}];"#,
        r#"string flag=1 [(common.field)={enum_number:"boolean"}];"#,
        r#"uint32 flag=1 [(common.field)={enum_str:"boolean"}];"#,
        r#"uint32 flag=1 [(common.field)={enum_number:"boolean" default:"2"}];"#,
        r#"string flag=1 [(common.field)={enum_str:"boolean" default:"true"}];"#,
        r#"uint64 flag=1 [(common.field)={default:"18446744073709551616"}];"#,
        r#"uint64 flag=1 [(common.field)={max_uint:18446744073709551614 default:"18446744073709551615"}];"#,
        r#"uint32 flag=1 [(common.field)={default:"-1"}];"#,
        r#"string flag=1 [(common.field)={max_uint:1}];"#,
        r#"uint32 flag=1 [(common.field)={min_uint:10 max_uint:1}];"#,
        r#"string flag=1 [(common.field)={template_ref:"missing"}];"#,
        r#"repeated string flag=1 [(common.field)={examples:'[""]' items:{min_length:1}}];"#,
        r#"string flag=1 [(common.field)={required:true}];"#,
    ] {
        let (temp, _) = fixture("", "");
        put(
            temp.path(),
            "proto/src/demo/job__list.proto",
            &format!(
                r#"syntax="proto3";import "common/api_options.proto";import "common/field_options.proto";option(common.api)={{title:"x" auth:SESSION}};message in{{{fields}}}message out{{}}"#
            ),
        );
        assert!(
            compiler::compile(&temp.path().join("proto/src"), "rust").is_err(),
            "accepted {fields}"
        );
    }
}
#[test]
fn get_sse_and_builtin_options_strip_without_losing_presence() {
    let (_t, w) = fixture(
        r#"optional string job_id=1 [deprecated=true,(common.field)={description:"任务"},json_name="jobId"];"#,
        "http:{method:GET} docs:{response_mode:SSE}",
    );
    let d = doc(&w);
    let op = &d["paths"]["/demo/job/list"]["get"];
    assert!(op.get("requestBody").is_none());
    assert_eq!(op["parameters"][0]["in"], "query");
    assert_eq!(op["parameters"][0]["required"], false);
    assert!(op["responses"]["200"]["content"]
        .get("text/event-stream")
        .is_some());
    let field = w
        .pool
        .get_message_by_name("proto.project.demo.job__list.in")
        .unwrap()
        .get_field_by_name("job_id")
        .unwrap();
    assert!(field.supports_presence());
    assert!(field
        .field_descriptor_proto()
        .options
        .as_ref()
        .unwrap()
        .deprecated());
}
#[test]
fn template_cycles_and_invalid_response_override_fail() {
    let (temp, _) = fixture("", "");
    put(
        temp.path(),
        "proto/src/common/doc_fields.proto",
        r#"syntax="proto3";import "common/field_options.proto";option(common.doc_fields)={templates:{name:"a" field:{template_ref:"b"}} templates:{name:"b" field:{description_ref:"a"}}};"#,
    );
    assert!(compiler::compile(&temp.path().join("proto/src"), "rust").is_err());
    let (_t, w) = fixture(
        "",
        r#"docs:{response_schema_json:'{"type":["array",null]}'}"#,
    );
    let p = serde_json::from_value(json!({"apifox_project_name":"test"})).unwrap();
    assert!(openapi::document(&w.endpoints[0], &w, &p).is_err());
}

#[test]
fn generated_php_validator_enforces_numeric_and_named_enums() {
    if std::process::Command::new("php")
        .arg("--version")
        .output()
        .is_err()
    {
        return;
    }
    let (temp, _) = fixture(
        r#"optional uint32 flag=1 [(common.field)={enum_number:"boolean"}]; string name=2 [(common.field)={enum_str:"boolean"}]; repeated uint32 flags=3 [(common.field)={enum_number:"boolean"}];"#,
        "",
    );
    put(temp.path(),"proto/generator.toml","version=1\ntarget=\"php\"\nsource=\"src\"\noutput=\"../gen/Proto\"\ncontrollers=\"../controllers\"\n");
    workspace_proto_gen::project::run(&temp.path().join("proto/generator.toml"), false).unwrap();
    let script = r#"$v=require $argv[1]; $type='Proto\\Project\\Demo\\job__list\\in';
foreach ([['name'=>'TRUE','flag'=>0],['name'=>'FALSE','flags'=>[0,1]]] as $input) { if ($v($type,$input)!==null) {exit(1);} }
foreach ([[],['name'=>'true'],['name'=>'TRUE','flag'=>2],['name'=>'TRUE','flag'=>true],['name'=>'TRUE','flags'=>[2]]] as $input) { if ($v($type,$input)===null) {exit(2);} }
"#;
    let output = std::process::Command::new("php")
        .arg("-r")
        .arg(script)
        .arg(temp.path().join("gen/Proto/Proto/enum_validator.php"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
