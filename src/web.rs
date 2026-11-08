//! Embedded local protocol browser, editor, request runner and Apifox publishing UI.
use crate::{
    apifox,
    compiler::{self, Config, Workspace},
    metadata_edit, openapi,
    settings::Settings,
};
use anyhow::{bail, ensure, Context, Result};
use axum::{
    extract::{DefaultBodyLimit, Multipart, Path as ApiPath, Query, Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use base64::Engine;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    hash::{Hash, Hasher},
    net::SocketAddr,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const RESPONSE_LIMIT: usize = 2 * 1024 * 1024;
const REQUEST_LIMIT: usize = 24 * 1024 * 1024;
struct Cached {
    fingerprint: u64,
    workspace: Workspace,
}
pub struct Project {
    config_path: PathBuf,
    config: Config,
    cache: Mutex<Option<Cached>>,
    lock: Mutex<()>,
}
#[derive(Clone)]
pub struct App {
    projects: Vec<Arc<Project>>,
    csrf: String,
    token: Option<String>,
    local_only: bool,
}
#[derive(Debug)]
pub struct WebError {
    status: StatusCode,
    message: String,
    completed: Vec<Value>,
}
#[derive(Debug)]
struct EditConflict;
impl std::fmt::Display for EditConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "文件已被外部修改，请刷新后再保存")
    }
}
impl std::error::Error for EditConflict {}
impl From<anyhow::Error> for WebError {
    fn from(e: anyhow::Error) -> Self {
        Self {
            status: if e.is::<EditConflict>() {
                StatusCode::CONFLICT
            } else {
                StatusCode::BAD_REQUEST
            },
            message: format!("{e:#}"),
            completed: vec![],
        }
    }
}
impl IntoResponse for WebError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({"error":self.message,"completed":self.completed})),
        )
            .into_response()
    }
}
type WebResult<T> = std::result::Result<T, WebError>;
fn error(status: StatusCode, message: &str) -> WebError {
    WebError {
        status,
        message: message.into(),
        completed: vec![],
    }
}
fn random_token() -> Result<String> {
    let mut b = [0u8; 32];
    getrandom::fill(&mut b).map_err(|e| anyhow::anyhow!("cannot create Web session token: {e}"))?;
    Ok(b.iter().map(|b| format!("{b:02x}")).collect())
}
fn files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = vec![];
    for entry in fs::read_dir(root)? {
        let p = entry?.path();
        if p.file_name().unwrap().to_string_lossy().starts_with('.') {
            continue;
        }
        if p.is_dir() {
            out.extend(files(&p)?)
        } else {
            out.push(p)
        }
    }
    out.sort();
    Ok(out)
}
fn fingerprint(p: &Project) -> Result<u64> {
    ensure!(
        serde_json::to_value(Config::load(&p.config_path)?)? == serde_json::to_value(&p.config)?,
        "生成配置已变更，请重启工作台"
    );
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for f in files(&p.config.source)? {
        f.hash(&mut h);
        fs::read(f)?.hash(&mut h)
    }
    let settings = Settings::load(p.config.source.parent().unwrap())?;
    for f in [
        p.config_path.clone(),
        p.config.source.parent().unwrap().join("config.json"),
        p.config.source.parent().unwrap().join(
            settings
                .enum_catalog
                .as_deref()
                .unwrap_or("enum_catalog.json"),
        ),
    ] {
        if f.is_file() {
            fs::read(f)?.hash(&mut h)
        }
    }
    Ok(h.finish())
}
fn snapshot<T>(p: &Project, f: impl FnOnce(&Workspace) -> Result<T>) -> Result<T> {
    let mut guard = p
        .cache
        .lock()
        .map_err(|_| anyhow::anyhow!("project cache is unavailable"))?;
    let hash = fingerprint(p)?;
    if guard.as_ref().is_none_or(|c| c.fingerprint != hash) {
        let workspace = compiler::compile(&p.config.source, &p.config.target)?;
        ensure!(hash == fingerprint(p)?, "协议正在被外部修改，请刷新重试");
        *guard = Some(Cached {
            fingerprint: hash,
            workspace,
        });
    }
    f(&guard.as_ref().unwrap().workspace)
}
fn selected<'a>(w: &'a Workspace, source: &str) -> Result<&'a compiler::Endpoint> {
    w.endpoints
        .iter()
        .find(|a| a.source == source)
        .context("协议不存在或不属于当前项目")
}
fn source_path(p: &Project, source: &str) -> Result<PathBuf> {
    ensure!(
        !Path::new(source).is_absolute()
            && Path::new(source)
                .components()
                .all(|c| matches!(c, Component::Normal(_))),
        "无效的协议路径"
    );
    let path = p.config.source.join(source).canonicalize()?;
    ensure!(
        path.starts_with(p.config.source.canonicalize()?),
        "协议路径超出项目目录"
    );
    Ok(path)
}
fn local_project() -> crate::settings::Project {
    serde_json::from_value(json!({"apifox_project_name":"Protocol Workbench"})).unwrap()
}
fn eligible(a: &compiler::Endpoint, p: &crate::settings::Project) -> bool {
    !p.apifox_token.is_empty()
        && if a.meta.export_targets.is_empty() {
            p.tag_list.iter().any(|t| a.meta.tags.contains(t))
        } else {
            a.meta
                .export_targets
                .iter()
                .any(|t| t == &p.name || t == &p.apifox_project_name)
        }
}
fn summary(a: &compiler::Endpoint) -> Value {
    json!({"source":a.source,"title":a.meta.title,"tags":a.meta.tags,"category_path":a.meta.category_path,"path":a.path,"method":if a.method().is_empty(){"POST"}else{a.method()},"auth":a.auth(),"deprecated":a.meta.deprecated,"export_targets":a.meta.export_targets})
}
fn detail(p: &Project, w: &Workspace, source: &str) -> Result<Value> {
    let a = selected(w, source)?;
    let text = fs::read_to_string(source_path(p, source)?)?;
    let mut out = summary(a);
    out["revision"] = json!(metadata_edit::revision(&text));
    out["source_text"] = json!(text);
    out["description"] = json!(a.meta.description);
    out["maintainers"] = json!(a.meta.maintainers);
    out["openapi"] = openapi::document(a, w, &local_project())?;
    let cfg = Settings::load(p.config.source.parent().unwrap())?;
    out["targets"]=json!(cfg.project_list.iter().enumerate().map(|(id,t)|json!({"id":id,"name":t.apifox_project_name,"key":t.name,"release_id":apifox::project_id(t,"release"),"test_id":apifox::project_id(t,"test"),"eligible":eligible(a,t),"configured":!t.apifox_token.is_empty(),"tags":t.tag_list})).collect::<Vec<_>>());
    Ok(out)
}
fn atomic_write(path: &Path, text: &str) -> Result<()> {
    use std::io::Write;
    let mut tmp = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
    tmp.as_file()
        .set_permissions(fs::metadata(path)?.permissions())?;
    tmp.write_all(text.as_bytes())?;
    tmp.flush()?;
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}
impl App {
    pub fn new(configs: &[PathBuf], token: Option<String>, local_only: bool) -> Result<Self> {
        ensure!(!configs.is_empty(), "at least one --config is required");
        let mut projects = vec![];
        let mut seen = BTreeSet::new();
        for path in configs {
            let path = path.canonicalize()?;
            if seen.insert(path.clone()) {
                let config = Config::load(&path)?;
                projects.push(Arc::new(Project {
                    config_path: path,
                    config,
                    cache: Mutex::new(None),
                    lock: Mutex::new(()),
                }));
            }
        }
        Ok(Self {
            projects,
            csrf: random_token()?,
            token,
            local_only,
        })
    }
    fn project(&self, id: usize) -> WebResult<Arc<Project>> {
        self.projects
            .get(id)
            .cloned()
            .ok_or_else(|| error(StatusCode::NOT_FOUND, "项目不存在"))
    }
    pub fn router(self) -> Router {
        Router::new()
            .route("/", get(index))
            .route("/app.js", get(js))
            .route("/style.css", get(css))
            .route("/api/projects", get(projects))
            .route("/api/projects/{id}/protocols", get(protocols))
            .route("/api/projects/{id}/protocol", get(protocol).patch(edit))
            .route("/api/projects/{id}/generate", post(generate))
            .route("/api/projects/{id}/request", post(request))
            .route("/api/projects/{id}/apifox/preview", post(preview))
            .route("/api/projects/{id}/apifox/sync", post(publish))
            .layer(DefaultBodyLimit::max(REQUEST_LIMIT))
            .layer(middleware::from_fn_with_state(self.clone(), guard))
            .with_state(self)
    }
}
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T> + Send + 'static,
) -> WebResult<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "后台任务异常"))?
        .map_err(WebError::from)
}
async fn guard(State(app): State<App>, req: Request, next: Next) -> Response {
    let host = req
        .headers()
        .get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if app.local_only {
        let name = host
            .parse::<axum::http::uri::Authority>()
            .ok()
            .map(|a| a.host().to_owned())
            .unwrap_or_default();
        if !["localhost", "127.0.0.1", "[::1]", "::1"].contains(&name.as_str()) {
            return error(StatusCode::FORBIDDEN, "仅允许本机访问").into_response();
        }
    }
    if req.uri().path().starts_with("/api/") {
        if let Some(token) = &app.token {
            let expected = format!("Bearer {token}");
            if req
                .headers()
                .get("authorization")
                .and_then(|h| h.to_str().ok())
                != Some(&expected)
            {
                return error(StatusCode::UNAUTHORIZED, "请输入工作台访问口令").into_response();
            }
        }
        if !matches!(*req.method(), Method::GET | Method::HEAD) {
            if req
                .headers()
                .get("x-proto-csrf")
                .and_then(|h| h.to_str().ok())
                != Some(&app.csrf)
            {
                return error(StatusCode::FORBIDDEN, "页面会话已失效，请刷新页面").into_response();
            }
            if let Some(origin) = req.headers().get("origin").and_then(|v| v.to_str().ok()) {
                let valid = reqwest::Url::parse(origin)
                    .ok()
                    .is_some_and(|url| url.authority() == host);
                if !valid {
                    return error(StatusCode::FORBIDDEN, "不允许跨站操作").into_response();
                }
            }
        }
    }
    let mut response = next.run(req).await;
    let h = response.headers_mut();
    h.insert("cache-control", HeaderValue::from_static("no-store"));
    h.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    h.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    h.insert("content-security-policy",HeaderValue::from_static("default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'"));
    response
}
async fn index(State(app): State<App>) -> Html<String> {
    Html(include_str!("../web/index.html").replace("__CSRF__", &app.csrf))
}
async fn js() -> impl IntoResponse {
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/javascript; charset=utf-8",
        )],
        include_str!("../web/app.js"),
    )
}
async fn css() -> impl IntoResponse {
    (
        [(axum::http::header::CONTENT_TYPE, "text/css; charset=utf-8")],
        include_str!("../web/style.css"),
    )
}
async fn projects(State(app): State<App>) -> Json<Value> {
    Json(
        json!({"version":env!("CARGO_PKG_VERSION"),"projects":app.projects.iter().enumerate().map(|(id,p)|json!({"id":id,"name":p.config.root().file_name().unwrap_or_default().to_string_lossy(),"target":p.config.target,"root":p.config.root()})).collect::<Vec<_>>()}),
    )
}
async fn protocols(State(app): State<App>, ApiPath(id): ApiPath<usize>) -> WebResult<Json<Value>> {
    let p = app.project(id)?;
    blocking(move || {
        snapshot(&p, |w| {
            Ok(Json(
                json!({"protocols":w.endpoints.iter().map(summary).collect::<Vec<_>>()}),
            ))
        })
    })
    .await
}
#[derive(Deserialize)]
struct Source {
    source: String,
}
async fn protocol(
    State(app): State<App>,
    ApiPath(id): ApiPath<usize>,
    Query(q): Query<Source>,
) -> WebResult<Json<Value>> {
    let p = app.project(id)?;
    blocking(move || snapshot(&p, |w| detail(&p, w, &q.source).map(Json))).await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Edit {
    source: String,
    revision: String,
    title: String,
    tags: Vec<String>,
}
async fn edit(
    State(app): State<App>,
    ApiPath(id): ApiPath<usize>,
    Json(input): Json<Edit>,
) -> WebResult<Json<Value>> {
    let p = app.project(id)?;
    blocking(move || {
        let _lock = p
            .lock
            .lock()
            .map_err(|_| anyhow::anyhow!("project edit unavailable"))?;
        snapshot(&p, |w| selected(w, &input.source).map(|_| ()))?;
        let path = source_path(&p, &input.source)?;
        let original = fs::read_to_string(&path)?;
        if metadata_edit::revision(&original) != input.revision {
            return Err(EditConflict.into());
        }
        let edited = metadata_edit::update(&original, &input.title, &input.tags)?;
        compiler::compile_with_overrides(
            &p.config.source,
            &p.config.target,
            &BTreeMap::from([(input.source.clone(), edited.clone())]),
        )?;
        if fs::read_to_string(&path)? != original {
            return Err(EditConflict.into());
        }
        atomic_write(&path, &edited)?;
        if let Err(e) = crate::project::run(&p.config_path, false) {
            if fs::read_to_string(&path).ok().as_deref() == Some(&edited) {
                atomic_write(&path, &original)?;
            }
            bail!("保存后生成失败，已保留原生成物并回退本次可回退的源修改: {e:#}")
        }
        *p.cache
            .lock()
            .map_err(|_| anyhow::anyhow!("cache unavailable"))? = None;
        snapshot(&p, |w| detail(&p, w, &input.source).map(Json))
    })
    .await
}
async fn generate(State(app): State<App>, ApiPath(id): ApiPath<usize>) -> WebResult<Json<Value>> {
    let p = app.project(id)?;
    blocking(move || {
        let _lock = p
            .lock
            .lock()
            .map_err(|_| anyhow::anyhow!("project edit unavailable"))?;
        fingerprint(&p)?;
        crate::project::run(&p.config_path, false)?;
        *p.cache
            .lock()
            .map_err(|_| anyhow::anyhow!("cache unavailable"))? = None;
        snapshot(&p, |w| Ok(Json(json!({"count":w.endpoints.len()}))))
    })
    .await
}
#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
struct Publish {
    sources: Vec<String>,
    env: String,
    targets: Option<Vec<usize>>,
}
struct Job {
    source: String,
    project: crate::settings::Project,
    id: String,
    document: Value,
}
fn jobs(p: &Project, input: &Publish) -> Result<Vec<Job>> {
    let cfg = Settings::load(p.config.source.parent().unwrap())?;
    snapshot(p, |w| {
        apifox::plan(
            w,
            &cfg,
            &input.sources,
            &input.env,
            input.targets.as_deref(),
        )
        .map(|jobs| {
            jobs.into_iter()
                .map(|(source, project, id, document)| Job {
                    source,
                    project: project.clone(),
                    id,
                    document,
                })
                .collect()
        })
    })
}
fn job_preview(job: &Job) -> Value {
    json!({"source":job.source,"project":job.project.apifox_project_name,"project_id":job.id,"paths":job.document["paths"],"document":job.document})
}
async fn preview(
    State(app): State<App>,
    ApiPath(id): ApiPath<usize>,
    Json(input): Json<Publish>,
) -> WebResult<Json<Value>> {
    let p = app.project(id)?;
    blocking(move || {
        let jobs = jobs(&p, &input)?;
        Ok(Json(
            json!({"env":input.env,"jobs":jobs.iter().map(job_preview).collect::<Vec<_>>()}),
        ))
    })
    .await
}
async fn publish(
    State(app): State<App>,
    ApiPath(id): ApiPath<usize>,
    Json(input): Json<Publish>,
) -> WebResult<Json<Value>> {
    let p = app.project(id)?;
    let jobs = blocking(move || jobs(&p, &input)).await?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(anyhow::Error::from)?;
    let mut completed = vec![];
    for job in jobs {
        if let Err(e) = apifox::sync(&client, &job.project, &job.id, &job.document).await {
            return Err(WebError {
                status: StatusCode::BAD_GATEWAY,
                message: format!(
                    "{} 同步结果未确认：{e:#}。已停止后续同步，请核对 Apifox；未自动重试。",
                    job.source
                ),
                completed,
            });
        }
        completed.push(json!({"source":job.source,"project":job.project.apifox_project_name,"project_id":job.id}));
    }
    Ok(Json(json!({"completed":completed})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TestRequest {
    source: String,
    base_url: String,
    path: String,
    method: String,
    query: BTreeMap<String, Value>,
    headers: BTreeMap<String, String>,
    body: String,
    content_type: String,
    timeout_seconds: u64,
}
struct FilePart {
    name: String,
    filename: String,
    mime: String,
    data: Vec<u8>,
}
async fn request(
    State(app): State<App>,
    ApiPath(id): ApiPath<usize>,
    mut multipart: Multipart,
) -> WebResult<Json<Value>> {
    let p = app.project(id)?;
    let mut input = None;
    let mut uploads = vec![];
    let mut size = 0;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| anyhow::anyhow!("请求内容解析失败: {e}"))?
    {
        let name = field.name().unwrap_or_default().to_owned();
        let filename = field.file_name().map(str::to_owned);
        let mime = field
            .content_type()
            .unwrap_or("application/octet-stream")
            .to_owned();
        let bytes = field
            .bytes()
            .await
            .map_err(|e| anyhow::anyhow!("读取上传内容失败: {e}"))?;
        size += bytes.len();
        if size > 20 * 1024 * 1024 {
            return Err(error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "调试请求最大支持 20 MiB",
            ));
        }
        if let Some(filename) = filename {
            uploads.push(FilePart {
                name,
                filename,
                mime,
                data: bytes.to_vec(),
            })
        } else if name == "request" {
            ensure_web(input.is_none(), "重复的调试请求配置")?;
            input =
                Some(serde_json::from_slice::<TestRequest>(&bytes).map_err(anyhow::Error::from)?);
        } else {
            return Err(error(StatusCode::BAD_REQUEST, "未知的调试表单字段"));
        }
    }
    let input = input.context("缺少调试请求配置")?;
    let source = input.source.clone();
    blocking(move || snapshot(&p, |w| selected(w, &source).map(|_| ()))).await?;
    execute_request(input, uploads)
        .await
        .map(Json)
        .map_err(WebError::from)
}
fn ensure_web(valid: bool, message: &str) -> WebResult<()> {
    if valid {
        Ok(())
    } else {
        Err(error(StatusCode::BAD_REQUEST, message))
    }
}
fn value_text(v: &Value) -> String {
    v.as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| v.to_string())
}
async fn execute_request(input: TestRequest, uploads: Vec<FilePart>) -> Result<Value> {
    ensure!(
        (1..=120).contains(&input.timeout_seconds),
        "超时范围为 1–120 秒"
    );
    let base =
        reqwest::Url::parse(input.base_url.trim()).context("服务地址必须是完整 http(s) URL")?;
    ensure!(
        ["http", "https"].contains(&base.scheme())
            && base.host_str().is_some()
            && base.username().is_empty()
            && base.password().is_none()
            && base.query().is_none()
            && base.fragment().is_none(),
        "无效的服务地址"
    );
    ensure!(
        input.path.starts_with('/')
            && !input.path.starts_with("//")
            && !input.path.contains(['\r', '\n', '#', '{', '}']),
        "请填写完整接口路径，并替换路径中的占位符"
    );
    let mut url = reqwest::Url::parse(&format!(
        "{}{}",
        base.as_str().trim_end_matches('/'),
        input.path
    ))?;
    ensure!(url.origin() == base.origin(), "接口路径不能改变服务地址");
    for (k, v) in &input.query {
        if !v.is_null() {
            url.query_pairs_mut().append_pair(k, &value_text(v));
        }
    }
    let method = reqwest::Method::from_bytes(input.method.as_bytes())?;
    ensure!(
        [
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
            Method::HEAD,
            Method::OPTIONS
        ]
        .contains(&method),
        "不支持此 HTTP 方法"
    );
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(input.timeout_seconds))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let mut headers = HeaderMap::new();
    for (k, v) in &input.headers {
        headers.insert(
            reqwest::header::HeaderName::from_bytes(k.as_bytes())?,
            HeaderValue::from_str(v)?,
        );
    }
    let mut builder = client.request(method.clone(), url.clone()).headers(headers);
    if input.content_type == "multipart/form-data" {
        ensure!(
            !matches!(method, Method::GET | Method::HEAD),
            "GET/HEAD 不支持 multipart body"
        );
        let values: BTreeMap<String, Value> =
            serde_json::from_str(&input.body).context("multipart 文本字段请填写 JSON 对象")?;
        let mut form = reqwest::multipart::Form::new();
        for (k, v) in values {
            if uploads.iter().any(|file| file.name == k) {
                continue;
            }
            if !v.is_null() {
                form = form.text(k, value_text(&v));
            }
        }
        for file in uploads {
            form = form.part(
                file.name,
                reqwest::multipart::Part::bytes(file.data)
                    .file_name(file.filename)
                    .mime_str(&file.mime)?,
            );
        }
        ensure!(
            !input
                .headers
                .keys()
                .any(|k| k.eq_ignore_ascii_case("content-type")),
            "multipart 的 Content-Type 会自动设置，请移除自定义的 Content-Type 请求头"
        );
        builder = builder.multipart(form);
    } else {
        ensure!(uploads.is_empty(), "文件仅能用于 multipart 请求");
        if !matches!(method, Method::GET | Method::HEAD) && !input.body.trim().is_empty() {
            if input.content_type.contains("json") {
                let _: Value =
                    serde_json::from_str(&input.body).context("请求 body 不是合法 JSON")?;
            }
            if !input
                .headers
                .keys()
                .any(|k| k.eq_ignore_ascii_case("content-type"))
            {
                builder = builder.header(reqwest::header::CONTENT_TYPE, &input.content_type);
            }
            builder = builder.body(input.body);
        }
    }
    let started = Instant::now();
    let mut response = builder.send().await.context("接口请求失败")?;
    let status = response.status().as_u16();
    let response_headers: BTreeMap<String, String> = response
        .headers()
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("<binary>").into()))
        .collect();
    let mut data = vec![];
    let mut partial = false;
    let deadline = tokio::time::Instant::now()
        + Duration::from_secs(input.timeout_seconds).saturating_sub(started.elapsed());
    loop {
        match tokio::time::timeout_at(deadline, response.chunk()).await {
            Ok(Ok(Some(chunk))) => {
                let take = (RESPONSE_LIMIT - data.len()).min(chunk.len());
                data.extend_from_slice(&chunk[..take]);
                if data.len() == RESPONSE_LIMIT {
                    partial = true;
                    break;
                }
            }
            Ok(Ok(None)) => break,
            Ok(Err(e)) => {
                if !data.is_empty() {
                    partial = true;
                    break;
                } else {
                    return Err(e).context("读取接口响应失败");
                }
            }
            Err(_) => {
                partial = true;
                break;
            }
        }
    }
    Ok(
        json!({"status":status,"headers":response_headers,"body":std::str::from_utf8(&data).map(str::to_owned).unwrap_or_else(|_|format!("二进制响应 · {} 字节，可点击下载响应查看原文件。",data.len())),"body_base64":base64::engine::general_purpose::STANDARD.encode(&data),"bytes":data.len(),"elapsed_ms":started.elapsed().as_millis(),"partial":partial,"url":url.as_str(),"method":method.as_str()}),
    )
}
pub async fn serve(
    configs: Vec<PathBuf>,
    listen: SocketAddr,
    token_env: Option<String>,
) -> Result<()> {
    let token = token_env
        .map(|key| std::env::var(&key).with_context(|| format!("missing {key}")))
        .transpose()?;
    ensure!(
        token.as_ref().is_none_or(|s| !s.trim().is_empty()),
        "Web access token is empty"
    );
    ensure!(
        listen.ip().is_loopback() || token.is_some(),
        "非本机监听需要 --token-env 指定访问口令环境变量"
    );
    let app = App::new(&configs, token, listen.ip().is_loopback())?;
    let listener = tokio::net::TcpListener::bind(listen).await?;
    println!("ProtoGen Web: http://{}", listener.local_addr()?);
    println!("Loaded {} project(s); Ctrl+C to stop.", app.projects.len());
    axum::serve(listener, app.router())
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        if let Ok(mut signal) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = signal.recv() => {} }
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}
