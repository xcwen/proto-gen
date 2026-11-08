//! Project-local request environments. These settings never enter generated metadata.
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};
pub const FILE: &str = "web.environments.local.json";
const MAX_TEXT: usize = 1024 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Configuration {
    pub version: u32,
    pub default_environment: String,
    pub environments: Vec<Environment>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Environment {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub variables: BTreeMap<String, String>,
    #[serde(default)]
    pub auth: Authentication,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Authentication {
    #[serde(rename = "type", default = "no_auth")]
    pub mode: String,
    #[serde(default)]
    pub header_name: String,
    #[serde(default)]
    pub value: String,
}
impl Default for Authentication {
    fn default() -> Self {
        Self {
            mode: "none".into(),
            header_name: String::new(),
            value: String::new(),
        }
    }
}
fn no_auth() -> String {
    "none".into()
}
#[derive(Debug)]
pub struct Conflict;
impl std::fmt::Display for Conflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "环境配置已变更，请重新载入后再操作")
    }
}
impl std::error::Error for Conflict {}
pub struct Store {
    pub config: Configuration,
    pub revision: String,
}
impl Default for Configuration {
    fn default() -> Self {
        Self {
            version: 1,
            default_environment: "release".into(),
            environments: [("release", "正式环境"), ("test", "测试环境")]
                .into_iter()
                .map(|(id, name)| Environment {
                    id: id.into(),
                    name: name.into(),
                    variables: BTreeMap::from([
                        ("API_BASE_URL".into(), String::new()),
                        ("TOKEN".into(), String::new()),
                        ("BEAR_TOKEN".into(), String::new()),
                    ]),
                    auth: Authentication {
                        mode: "none".into(),
                        ..Default::default()
                    },
                })
                .collect(),
        }
    }
}
fn variable_name(name: &str) -> bool {
    let mut b = name.bytes();
    b.next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == b'_')
        && b.all(|c| c.is_ascii_alphanumeric() || c == b'_')
}
impl Configuration {
    pub fn validate(&self) -> Result<()> {
        ensure!(self.version == 1, "不支持的环境配置版本");
        ensure!(self.environments.len() <= 32, "最多支持 32 个环境");
        let mut ids = BTreeSet::new();
        for e in &self.environments {
            ensure!(
                !e.id.is_empty()
                    && e.id.len() <= 80
                    && e.id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
                    && ids.insert(&e.id),
                "环境 ID 为空、重复或无效"
            );
            ensure!(
                !e.name.trim().is_empty() && e.name.chars().count() <= 120,
                "环境名称不能为空且不能超过 120 字符"
            );
            ensure!(e.variables.len() <= 256, "单个环境最多支持 256 个变量");
            for (k, v) in &e.variables {
                ensure!(
                    variable_name(k) && k.len() <= 120,
                    "无效的环境变量名称：{k}"
                );
                ensure!(v.len() <= 65536, "环境变量 {k} 的值过长");
            }
            e.auth.validate(false)?;
        }
        ensure!(
            self.default_environment.is_empty() || ids.contains(&self.default_environment),
            "默认环境不存在"
        );
        Ok(())
    }
    pub fn environment(&self, id: &str) -> Result<&Environment> {
        self.environments
            .iter()
            .find(|e| e.id == id)
            .context("所选环境不存在，请重新选择")
    }
}
impl Authentication {
    pub fn validate(&self, inherit: bool) -> Result<()> {
        ensure!(
            ["none", "header_token", "header_bear_auth"].contains(&self.mode.as_str())
                || (inherit && self.mode == "environment"),
            "不支持的认证方式"
        );
        ensure!(
            self.header_name.len() <= 256 && self.value.len() <= 65536,
            "认证配置过长"
        );
        Ok(())
    }
}
pub fn load(proto: &Path) -> Result<Store> {
    let file = proto.join(FILE);
    let text = if file.exists() {
        ensure!(
            fs::metadata(&file)?.len() <= MAX_TEXT as u64,
            "环境配置文件过大"
        );
        fs::read_to_string(file)?
    } else {
        serde_json::to_string_pretty(&Configuration::default())?
    };
    let config: Configuration = serde_json::from_str(&text).context("环境配置 JSON 无效")?;
    config.validate()?;
    Ok(Store {
        config,
        revision: crate::metadata_edit::revision(&text),
    })
}
pub fn save(proto: &Path, revision: &str, config: Configuration) -> Result<Store> {
    use std::io::Write;
    config.validate()?;
    if load(proto)?.revision != revision {
        return Err(Conflict.into());
    }
    let mut text = serde_json::to_string_pretty(&config)?;
    text.push('\n');
    ensure!(text.len() <= MAX_TEXT, "环境配置文件过大");
    let path = proto.join(FILE);
    let mut temp = tempfile::NamedTempFile::new_in(proto)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temp.as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    temp.write_all(text.as_bytes())?;
    temp.flush()?;
    if load(proto)?.revision != revision {
        return Err(Conflict.into());
    }
    temp.persist(path).map_err(|e| e.error)?;
    load(proto)
}
fn render_inner(
    text: &str,
    vars: &BTreeMap<String, String>,
    stack: &mut Vec<String>,
) -> Result<String> {
    ensure!(stack.len() < 24, "环境变量引用过深");
    let mut result = String::new();
    let mut offset = 0;
    while offset < text.len() {
        let rest = &text[offset..];
        if rest.starts_with("\\{{") {
            let end = rest.find("}}").context("环境变量占位符缺少 }}")?;
            result.push_str(&rest[1..end + 2]);
            offset += end + 2;
        } else if rest.starts_with("{{") {
            let end = rest.find("}}").context("环境变量占位符缺少 }}")?;
            let name = rest[2..end].trim();
            ensure!(variable_name(name), "无效的环境变量引用");
            ensure!(!stack.iter().any(|k| k == name), "环境变量循环引用：{name}");
            let raw = vars
                .get(name)
                .with_context(|| format!("环境变量未定义：{name}"))?;
            stack.push(name.into());
            result.push_str(&render_inner(raw, vars, stack)?);
            stack.pop();
            offset += end + 2;
        } else {
            let c = rest.chars().next().unwrap();
            result.push(c);
            offset += c.len_utf8();
        }
        ensure!(result.len() <= MAX_TEXT, "环境变量展开后内容过大");
    }
    Ok(result)
}
pub fn render(text: &str, vars: &BTreeMap<String, String>) -> Result<String> {
    render_inner(text, vars, &mut vec![])
}
pub fn render_value(value: Value, vars: &BTreeMap<String, String>) -> Result<Value> {
    Ok(match value {
        Value::String(s) => json!(render(&s, vars)?),
        Value::Array(xs) => Value::Array(
            xs.into_iter()
                .map(|x| render_value(x, vars))
                .collect::<Result<_>>()?,
        ),
        Value::Object(xs) => {
            let mut out = serde_json::Map::new();
            for (k, v) in xs {
                let key = render(&k, vars)?;
                ensure!(!out.contains_key(&key), "变量展开后 JSON 字段重名");
                out.insert(key, render_value(v, vars)?);
            }
            Value::Object(out)
        }
        other => other,
    })
}
pub fn render_body(
    body: &str,
    content_type: &str,
    vars: &BTreeMap<String, String>,
) -> Result<String> {
    if body.trim().is_empty() {
        return Ok(body.into());
    }
    if !content_type.contains("json") && content_type != "multipart/form-data" {
        return render(body, vars);
    }
    // Quoted variables stay strings. Bare variables become typed JSON values.
    let mut nonce = [0; 16];
    getrandom::fill(&mut nonce).map_err(|e| anyhow::anyhow!("cannot initialize template: {e}"))?;
    let prefix = format!(
        "__proto_gen_{}_",
        nonce.iter().map(|b| format!("{b:02x}")).collect::<String>()
    );
    let mut prepared = String::new();
    let mut symbols = BTreeMap::new();
    let mut quoted = false;
    let mut escaped = false;
    let mut offset = 0;
    while offset < body.len() {
        let rest = &body[offset..];
        if !quoted && rest.starts_with("{{") {
            let end = rest.find("}}").context("环境变量占位符缺少 }}")?;
            let name = rest[2..end].trim();
            ensure!(variable_name(name), "无效的环境变量引用");
            let resolved = render(&format!("{{{{{name}}}}}"), vars)?;
            let value: Value = serde_json::from_str(&resolved).with_context(|| {
                format!("变量 {name} 用于 JSON 值时必须是合法 JSON；字符串变量请放在引号内")
            })?;
            let key = format!("{prefix}{}", symbols.len());
            prepared.push_str(&serde_json::to_string(&key)?);
            symbols.insert(key, value);
            offset += end + 2;
            continue;
        }
        let c = rest.chars().next().unwrap();
        prepared.push(c);
        if quoted {
            if escaped {
                escaped = false
            } else if c == '\\' {
                escaped = true
            } else if c == '"' {
                quoted = false
            }
        } else if c == '"' {
            quoted = true
        }
        offset += c.len_utf8();
    }
    fn walk(
        v: Value,
        vars: &BTreeMap<String, String>,
        symbols: &BTreeMap<String, Value>,
    ) -> Result<Value> {
        if let Value::String(s) = &v {
            if let Some(value) = symbols.get(s) {
                return Ok(value.clone());
            }
        }
        match v {
            Value::Array(xs) => Ok(Value::Array(
                xs.into_iter()
                    .map(|v| walk(v, vars, symbols))
                    .collect::<Result<_>>()?,
            )),
            Value::Object(xs) => {
                let mut out = serde_json::Map::new();
                for (k, v) in xs {
                    let key = render(&k, vars)?;
                    ensure!(!out.contains_key(&key), "变量展开后 JSON 字段重名");
                    out.insert(key, walk(v, vars, symbols)?);
                }
                Ok(Value::Object(out))
            }
            v => render_value(v, vars),
        }
    }
    let value: Value = serde_json::from_str(&prepared).context("请求 body 不是合法 JSON")?;
    Ok(serde_json::to_string(&walk(value, vars, &symbols)?)?)
}
pub fn authentication(
    headers: &mut BTreeMap<String, String>,
    auth: &Authentication,
    vars: &BTreeMap<String, String>,
) -> Result<()> {
    auth.validate(false)?;
    if auth.mode == "none" {
        return Ok(());
    }
    let header = if auth.header_name.trim().is_empty() {
        if auth.mode == "header_bear_auth" {
            "Authorization"
        } else {
            "token"
        }
    } else {
        auth.header_name.trim()
    };
    let name = render(header, vars)?;
    reqwest::header::HeaderName::from_bytes(name.as_bytes()).context("认证请求头名称无效")?;
    let value = render(&auth.value, vars)?;
    ensure!(
        !value.contains(['\r', '\n']),
        "认证 Token 包含非法请求头字符"
    );
    let value = value.trim();
    ensure!(!value.is_empty(), "认证 Token 为空，请配置对应环境变量");
    let value = if auth.mode == "header_bear_auth" {
        if value
            .get(..7)
            .is_some_and(|p| p.eq_ignore_ascii_case("bearer "))
        {
            format!("Bearer {}", value[7..].trim())
        } else {
            format!("Bearer {value}")
        }
    } else {
        value.to_owned()
    };
    ensure!(
        auth.mode != "header_bear_auth" || !value[7..].trim().is_empty(),
        "Bearer Token 为空"
    );
    reqwest::header::HeaderValue::from_str(&value).context("认证 Token 包含非法请求头字符")?;
    headers.retain(|key, _| !key.eq_ignore_ascii_case(&name));
    headers.insert(name, value);
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn variables_json_types_escaping_and_authentication() {
        let vars = BTreeMap::from([
            ("HOST".into(), "https://example.test".into()),
            ("API_BASE_URL".into(), "{{HOST}}/api".into()),
            ("TOKEN".into(), "a\"b\\c".into()),
            ("LIMIT".into(), "10".into()),
            ("BEAR_TOKEN".into(), "abc".into()),
        ]);
        assert_eq!(
            render("{{API_BASE_URL}}/list", &vars).unwrap(),
            "https://example.test/api/list"
        );
        let body = render_body(
            r#"{"token":"{{TOKEN}}","limit":{{LIMIT}},"literal":"\\{{TOKEN}}"}"#,
            "application/json",
            &vars,
        )
        .unwrap();
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["token"], "a\"b\\c");
        assert_eq!(v["limit"], 10);
        assert_eq!(v["literal"], "{{TOKEN}}");
        let mut headers = BTreeMap::from([("authorization".into(), "old".into())]);
        authentication(
            &mut headers,
            &Authentication {
                mode: "header_bear_auth".into(),
                header_name: String::new(),
                value: "{{BEAR_TOKEN}}".into(),
            },
            &vars,
        )
        .unwrap();
        assert_eq!(headers.len(), 1);
        assert_eq!(headers["Authorization"], "Bearer abc");
        authentication(
            &mut headers,
            &Authentication {
                mode: "header_token".into(),
                header_name: "X-Token".into(),
                value: "{{BEAR_TOKEN}}".into(),
            },
            &vars,
        )
        .unwrap();
        assert_eq!(headers["X-Token"], "abc");
    }
    #[test]
    fn missing_cyclic_and_invalid_auth_values_fail() {
        assert!(render("{{NO}}", &BTreeMap::new()).is_err());
        assert!(render(
            "{{A}}",
            &BTreeMap::from([("A".into(), "{{B}}".into()), ("B".into(), "{{A}}".into())])
        )
        .is_err());
        assert!(authentication(
            &mut BTreeMap::new(),
            &Authentication {
                mode: "header_token".into(),
                header_name: "token".into(),
                value: "\r\ninvalid".into()
            },
            &BTreeMap::new()
        )
        .is_err());
    }
    #[test]
    fn local_settings_roundtrip_and_conflict() {
        let d = tempfile::tempdir().unwrap();
        let initial = load(d.path()).unwrap();
        assert!(!d.path().join(FILE).exists());
        let mut config = initial.config.clone();
        config.environments[0]
            .variables
            .insert("TOKEN".into(), "local-secret".into());
        let stored = save(d.path(), &initial.revision, config.clone()).unwrap();
        assert_eq!(
            stored.config.environments[0].variables["TOKEN"],
            "local-secret"
        );
        assert!(save(d.path(), &initial.revision, config).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(d.path().join(FILE))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }
}
