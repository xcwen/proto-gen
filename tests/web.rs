use serde_json::{json, Value};
use std::{collections::BTreeMap, fs, path::Path};
use workspace_proto_gen::{compiler, web::App};
fn put(root: &Path, name: &str, value: &str) {
    let p = root.join(name);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, value).unwrap();
}
fn fixture() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    put(
        d.path(),
        "proto/src/common/api_options.proto",
        compiler::SCHEMA,
    );
    put(
        d.path(),
        "proto/src/common/field_options.proto",
        workspace_proto_gen::field_model::SCHEMA,
    );
    put(d.path(),"proto/generator.toml","version=1\ntarget=\"php\"\nsource=\"src\"\noutput=\"../gen/Proto\"\ncontrollers=\"../controllers\"\n");
    put(
        d.path(),
        "proto/config.json",
        r#"{"project_list":[{"name":"fixture","apifox_project_name":"fixture","apifox_token":"do-not-leak-this-token","apifox_project_id":"123","test_apifox_project_id":"456","tag_list":["demo"],"url_replace_cfg":{"/^/demo//":"/v1/"}}]}"#,
    );
    put(
        d.path(),
        "proto/src/demo/job__list.proto",
        r#"syntax="proto3";import "common/api_options.proto";import "common/field_options.proto";
option(common.api)={title:"列表" auth:SESSION tags:"demo" maintainers:"jim" docs:{status:"testing"} description:'a title: "literal"'};
message in{ optional uint32 limit=1 [(common.field)={default:"10"}]; }
message out{uint64 total=1;}
"#,
    );
    d
}
async fn server(app: App) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, app.router()).await.unwrap();
    });
    (base, task)
}
async fn csrf(client: &reqwest::Client, base: &str) -> String {
    let html = client.get(base).send().await.unwrap().text().await.unwrap();
    regex::Regex::new(r#"name="csrf-token" content="([a-f0-9]+)""#)
        .unwrap()
        .captures(&html)
        .unwrap()[1]
        .into()
}
#[tokio::test]
async fn browse_edit_generate_conflicts_and_preview_share_the_native_model() {
    let d = fixture();
    let config = d.path().join("proto/generator.toml");
    let (base, task) = server(App::new(std::slice::from_ref(&config), None, true).unwrap()).await;
    let client = reqwest::Client::new();
    let token = csrf(&client, &base).await;
    let list = client
        .get(format!("{base}/api/projects/0/protocols"))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(list["protocols"][0]["title"], "列表");
    let url = format!("{base}/api/projects/0/protocol?source=demo/job__list.proto");
    let detail = client
        .get(&url)
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(detail["openapi"]["openapi"], "3.1.0");
    assert_eq!(list["workspace_revision"], detail["workspace_revision"]);
    assert!(list["workspace_revision"].is_string());
    assert!(!detail.to_string().contains("do-not-leak-this-token"));
    let patch = json!({"source":"demo/job__list.proto","revision":detail["revision"],"title":"新的标题 \"引号\"","tags":["demo","new"]});
    let blocked = client.patch(&url).json(&patch).send().await.unwrap();
    assert_eq!(blocked.status(), 403);
    let saved = client
        .patch(&url)
        .header("x-proto-csrf", &token)
        .json(&patch)
        .send()
        .await
        .unwrap();
    assert_eq!(saved.status(), 200);
    let saved = saved.json::<Value>().await.unwrap();
    assert_eq!(saved["title"], "新的标题 \"引号\"");
    assert_eq!(saved["tags"], json!(["demo", "new"]));
    let source = fs::read_to_string(d.path().join("proto/src/demo/job__list.proto")).unwrap();
    assert!(source.contains("auth:SESSION"));
    assert!(source.contains("status:\"testing\""));
    assert!(source.contains("a title: \"literal\""));
    assert!(source.contains("optional uint32 limit=1"));
    assert!(d.path().join("gen/Proto/Proto/cmd_list.php").exists());
    workspace_proto_gen::project::run(&config, true).unwrap();
    assert_eq!(
        client
            .patch(&url)
            .header("x-proto-csrf", &token)
            .json(&patch)
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    let response = client
        .post(format!("{base}/api/projects/0/apifox/preview"))
        .header("x-proto-csrf", &token)
        .json(&json!({"workspace_revision":saved["workspace_revision"],"sources":["demo/job__list.proto"],"env":"test","targets":[0]}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let preview = response.json::<Value>().await.unwrap();
    assert_eq!(preview["jobs"][0]["project_id"], "456");
    assert!(preview["jobs"][0]["document"]["paths"]
        .get("/v1/job/list")
        .is_some());
    assert!(!preview.to_string().contains("do-not-leak-this-token"));
    let config_file = d.path().join("proto/config.json");
    let changed = fs::read_to_string(&config_file)
        .unwrap()
        .replace("123", "789");
    fs::write(config_file, changed).unwrap();
    let stale=client.post(format!("{base}/api/projects/0/apifox/preview")).header("x-proto-csrf",&token).json(&json!({"workspace_revision":saved["workspace_revision"],"sources":["demo/job__list.proto"],"env":"test","targets":[0]})).send().await.unwrap();
    assert_eq!(stale.status(), 409);

    assert_eq!(
        client
            .get(format!(
                "{base}/api/projects/0/protocol?source=../../outside.proto"
            ))
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    assert_eq!(
        client
            .get(format!("{base}/api/projects"))
            .header("host", "evil.example")
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    task.abort();
}
#[tokio::test]
async fn invalid_controller_rolls_back_the_source_and_generated_output() {
    let d = fixture();
    let config = d.path().join("proto/generator.toml");
    workspace_proto_gen::project::run(&config, false).unwrap();
    let source = d.path().join("proto/src/demo/job__list.proto");
    let original = fs::read(&source).unwrap();
    let generated = fs::read(d.path().join("gen/Proto/cmd.json")).unwrap();
    put(d.path(), "controllers/Demo/Job.php", "<?php class Job {");
    let (base, task) = server(App::new(&[config], None, true).unwrap()).await;
    let client = reqwest::Client::new();
    let token = csrf(&client, &base).await;
    let url = format!("{base}/api/projects/0/protocol?source=demo/job__list.proto");
    let detail = client
        .get(&url)
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    let response=client.patch(&url).header("x-proto-csrf",token).json(&json!({"source":"demo/job__list.proto","revision":detail["revision"],"title":"不能安装","tags":["demo"]})).send().await.unwrap();
    assert_eq!(response.status(), 400);
    assert_eq!(fs::read(source).unwrap(), original);
    assert_eq!(
        fs::read(d.path().join("gen/Proto/cmd.json")).unwrap(),
        generated
    );
    task.abort();
}
#[tokio::test]
async fn access_token_is_required_when_configured() {
    let d = fixture();
    let (base, task) = server(
        App::new(
            &[d.path().join("proto/generator.toml")],
            Some("test-access".into()),
            false,
        )
        .unwrap(),
    )
    .await;
    let client = reqwest::Client::new();
    assert_eq!(
        client
            .get(format!("{base}/api/projects"))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        client
            .get(format!("{base}/api/projects"))
            .bearer_auth("test-access")
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    task.abort();
}
#[tokio::test]
async fn request_runner_preserves_query_body_headers_and_http_failures() {
    use axum::{extract::Request, routing::any, Router};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream = format!("http://{}/prefix", listener.local_addr().unwrap());
    let echo = tokio::spawn(async move {
        axum::serve(listener,Router::new().fallback(any(|req:Request|async move{let method=req.method().to_string();let uri=req.uri().to_string();let auth=req.headers().get("authorization").unwrap().to_str().unwrap().to_owned();let body=axum::body::to_bytes(req.into_body(),1024*1024).await.unwrap();(axum::http::StatusCode::UNAUTHORIZED,axum::Json(json!({"method":method,"uri":uri,"authorization":auth,"body":String::from_utf8_lossy(&body)})))}))).await.unwrap()
    });
    let d = fixture();
    let (base, task) =
        server(App::new(&[d.path().join("proto/generator.toml")], None, true).unwrap()).await;
    let client = reqwest::Client::new();
    let token = csrf(&client, &base).await;
    for method in ["POST", "GET"] {
        let config = json!({"source":"demo/job__list.proto","base_url":upstream,"path":"/echo","method":method,"query":{"zero":0,"flag":false},"headers":{"Authorization":"Bearer dummy"},"body":"{\"id\":\"18446744073709551615\"}","content_type":"application/json","timeout_seconds":3});
        let form = reqwest::multipart::Form::new().text("request", config.to_string());
        let result = client
            .post(format!("{base}/api/projects/0/request"))
            .header("x-proto-csrf", &token)
            .multipart(form)
            .send()
            .await
            .unwrap();
        assert_eq!(result.status(), 200);
        let result = result.json::<Value>().await.unwrap();
        assert_eq!(result["status"], 401);
        let echoed: Value = serde_json::from_str(result["body"].as_str().unwrap()).unwrap();
        assert_eq!(echoed["uri"], "/prefix/echo?flag=false&zero=0");
        assert_eq!(echoed["authorization"], "Bearer dummy");
        assert_eq!(echoed["method"], method);
        assert_eq!(
            echoed["body"],
            if method == "GET" {
                ""
            } else {
                "{\"id\":\"18446744073709551615\"}"
            }
        );
    }
    task.abort();
    echo.abort();
}

#[tokio::test]
async fn multipart_and_redirect_responses_are_not_rewritten() {
    use axum::{extract::Request, routing::any, Router};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream = format!("http://{}", listener.local_addr().unwrap());
    let echo = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().fallback(any(|req: Request| async move {
                let content_type = req
                    .headers()
                    .get("content-type")
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_owned();
                let body = axum::body::to_bytes(req.into_body(), 1024 * 1024)
                    .await
                    .unwrap();
                (
                    axum::http::StatusCode::FOUND,
                    [("location", "/do-not-follow")],
                    axum::Json(
                        json!({"content_type":content_type,"body":String::from_utf8_lossy(&body)}),
                    ),
                )
            })),
        )
        .await
        .unwrap()
    });
    let d = fixture();
    let (base, task) =
        server(App::new(&[d.path().join("proto/generator.toml")], None, true).unwrap()).await;
    let client = reqwest::Client::new();
    let token = csrf(&client, &base).await;
    let config = json!({"source":"demo/job__list.proto","base_url":upstream,"path":"/echo","method":"POST","query":{},"headers":{},"body":"{\"file\":\"\",\"flag\":false,\"limit\":0}","content_type":"multipart/form-data","timeout_seconds":3});
    let form = reqwest::multipart::Form::new()
        .text("request", config.to_string())
        .part(
            "file",
            reqwest::multipart::Part::bytes(b"file-content".to_vec())
                .file_name("test.txt")
                .mime_str("text/plain")
                .unwrap(),
        );
    let result = client
        .post(format!("{base}/api/projects/0/request"))
        .header("x-proto-csrf", token)
        .multipart(form)
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(result["status"], 302);
    assert_eq!(result["headers"]["location"], "/do-not-follow");
    let body: Value = serde_json::from_str(result["body"].as_str().unwrap()).unwrap();
    assert!(body["content_type"]
        .as_str()
        .unwrap()
        .contains("multipart/form-data; boundary="));
    let payload = body["body"].as_str().unwrap();
    assert!(payload.contains("file-content"));
    assert!(payload.contains("\r\nfalse\r\n"));
    assert!(payload.contains("\r\n0\r\n"));
    assert_eq!(payload.matches("name=\"file\"").count(), 1);
    task.abort();
    echo.abort();
}

#[tokio::test]
async fn streaming_preview_returns_partial_data_at_the_wait_limit() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream = format!("http://{}", listener.local_addr().unwrap());
    let stream = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buffer = [0; 4096];
        let _ = socket.read(&mut buffer).await;
        let event = "data: first\n\n";
        socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{}\r\n",event.len(),event).as_bytes()).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    });
    let d = fixture();
    let (base, task) =
        server(App::new(&[d.path().join("proto/generator.toml")], None, true).unwrap()).await;
    let client = reqwest::Client::new();
    let token = csrf(&client, &base).await;
    let config = json!({"source":"demo/job__list.proto","base_url":upstream,"path":"/events","method":"GET","query":{},"headers":{},"body":"","content_type":"application/json","timeout_seconds":1});
    let form = reqwest::multipart::Form::new().text("request", config.to_string());
    let result = client
        .post(format!("{base}/api/projects/0/request"))
        .header("x-proto-csrf", token)
        .multipart(form)
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(result["status"], 200);
    assert_eq!(result["partial"], true);
    assert_eq!(result["body"], "data: first\n\n");
    task.abort();
    stream.abort();
}

#[tokio::test]
async fn environment_variables_authentication_and_local_persistence() {
    use axum::{extract::Request, routing::any, Router};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream = format!("http://{}", listener.local_addr().unwrap());
    let echo = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().fallback(any(|req: Request| async move {
                let uri = req.uri().to_string();
                let headers: BTreeMap<String, String> = req
                    .headers()
                    .iter()
                    .filter(|(k, _)| {
                        ["token", "authorization", "x-token", "x-note"].contains(&k.as_str())
                    })
                    .map(|(k, v)| (k.to_string(), v.to_str().unwrap().into()))
                    .collect();
                let bytes = axum::body::to_bytes(req.into_body(), 1024 * 1024)
                    .await
                    .unwrap();
                axum::Json(
                    json!({"uri":uri,"headers":headers,"body":String::from_utf8_lossy(&bytes)}),
                )
            })),
        )
        .await
        .unwrap()
    });
    let d = fixture();
    let config = d.path().join("proto/generator.toml");
    let (base, task) = server(App::new(std::slice::from_ref(&config), None, true).unwrap()).await;
    let client = reqwest::Client::new();
    let token = csrf(&client, &base).await;
    let url = format!("{base}/api/projects/0/environments");
    let initial = client
        .get(&url)
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(initial["config"]["environments"][0]["name"], "正式环境");
    assert!(!d.path().join("proto/web.environments.local.json").exists());
    let mut settings = initial["config"].clone();
    settings["environments"][0]["variables"] = json!({"API_BASE_URL":upstream,"TOKEN":"release-token","BEAR_TOKEN":"release-bear","TEXT":"quote \" and slash","LIMIT":"10","ACTIVE":"false","NEXT":"{{TOKEN}}"});
    settings["environments"][0]["auth"] =
        json!({"type":"header_token","header_name":"token","value":"{{TOKEN}}"});
    settings["environments"][1]["variables"] =
        json!({"API_BASE_URL":upstream,"TOKEN":"test-token","BEAR_TOKEN":"test-bear"});
    settings["environments"][1]["auth"] =
        json!({"type":"header_bear_auth","header_name":"Authorization","value":"{{BEAR_TOKEN}}"});
    let update = json!({"revision":initial["revision"],"config":settings});
    assert_eq!(
        client
            .put(&url)
            .json(&update)
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    let saved = client
        .put(&url)
        .header("x-proto-csrf", &token)
        .json(&update)
        .send()
        .await
        .unwrap();
    assert_eq!(saved.status(), 200);
    let saved = saved.json::<Value>().await.unwrap();
    assert_eq!(
        client
            .put(&url)
            .header("x-proto-csrf", &token)
            .json(&update)
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    let url_response = client.post(format!("{base}/api/projects/0/url-preview")).header("x-proto-csrf",&token)
        .json(&json!({"base_url":"{{API_BASE_URL}}","path":"/echo","query":{"limit":0,"next":"{{NEXT}}"},"environment":"release","environment_revision":saved["revision"]})).send().await.unwrap();
    assert_eq!(url_response.status(), 200);
    let url_preview = url_response.json::<Value>().await.unwrap();
    assert_eq!(url_preview["base_url"], upstream);
    assert_eq!(
        url_preview["url"],
        format!("{upstream}/echo?limit=0&next=release-token")
    );
    let on_disk = fs::read_to_string(d.path().join("proto/web.environments.local.json")).unwrap();
    assert!(on_disk.contains("release-token"));
    async fn send(
        client: &reqwest::Client,
        base: &str,
        token: &str,
        input: Value,
    ) -> reqwest::Response {
        client
            .post(format!("{base}/api/projects/0/request"))
            .header("x-proto-csrf", token)
            .multipart(reqwest::multipart::Form::new().text("request", input.to_string()))
            .send()
            .await
            .unwrap()
    }
    let input = json!({"source":"demo/job__list.proto","base_url":"{{API_BASE_URL}}","path":"/echo","method":"POST","query":{"next":"{{NEXT}}"},"headers":{"X-Note":"{{TOKEN}}"},"body":"{\"token\":\"{{TOKEN}}\",\"text\":\"{{TEXT}}\",\"limit\":{{LIMIT}},\"active\":{{ACTIVE}}}","content_type":"application/json","timeout_seconds":3,"environment":"release","environment_revision":saved["revision"],"auth":{"type":"environment"}});
    let result = send(&client, &base, &token, input.clone())
        .await
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(result["status"], 200);
    let body: Value = serde_json::from_str(result["body"].as_str().unwrap()).unwrap();
    assert_eq!(body["headers"]["token"], "release-token");
    assert_eq!(body["headers"]["x-note"], "release-token");
    let payload: Value = serde_json::from_str(body["body"].as_str().unwrap()).unwrap();
    assert_eq!(payload["limit"], 10);
    assert_eq!(payload["active"], false);
    assert_eq!(payload["text"], "quote \" and slash");
    let mut test_input = input.clone();
    test_input["environment"] = json!("test");
    test_input["query"] = json!({});
    test_input["headers"] = json!({});
    test_input["body"] = json!("{}");
    let result = send(&client, &base, &token, test_input.clone())
        .await
        .json::<Value>()
        .await
        .unwrap();
    let body: Value = serde_json::from_str(result["body"].as_str().unwrap()).unwrap();
    assert_eq!(body["headers"]["authorization"], "Bearer test-bear");
    assert!(body["headers"].get("token").is_none());
    test_input["auth"] = json!({"type":"header_token","header_name":"X-Token","value":"{{TOKEN}}"});
    let result = send(&client, &base, &token, test_input.clone())
        .await
        .json::<Value>()
        .await
        .unwrap();
    let body: Value = serde_json::from_str(result["body"].as_str().unwrap()).unwrap();
    assert_eq!(body["headers"]["x-token"], "test-token");
    assert!(body["headers"].get("authorization").is_none());
    test_input["auth"] = json!({"type":"none"});
    let result = send(&client, &base, &token, test_input.clone())
        .await
        .json::<Value>()
        .await
        .unwrap();
    let body: Value = serde_json::from_str(result["body"].as_str().unwrap()).unwrap();
    assert!(body["headers"].as_object().unwrap().is_empty());
    test_input["base_url"] = json!("{{MISSING}}");
    assert_eq!(
        send(&client, &base, &token, test_input.clone())
            .await
            .status(),
        400
    );
    test_input["environment_revision"] = initial["revision"].clone();
    assert_eq!(send(&client, &base, &token, test_input).await.status(), 409);
    workspace_proto_gen::project::run(&config, false).unwrap();
    let generated = fs::read_to_string(d.path().join("gen/Proto/info.json")).unwrap();
    assert!(!generated.contains("release-token"));
    assert!(!generated.contains("API_BASE_URL"));
    task.abort();
    echo.abort();
}
