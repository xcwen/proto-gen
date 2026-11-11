//! Build-time authorization compilation. Documentation export settings never grant access.
use crate::compiler::Endpoint;
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorizationConfig {
    pub schema_version: u32,
    pub enforced_route_prefixes: Vec<String>,
    pub enforced_source_groups: Vec<String>,
    pub projects: BTreeMap<String, ProjectDefinition>,
    pub policies: BTreeMap<String, ProviderPolicy>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProjectDefinition {
    pub tenant_id: u32,
    pub project_type: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorizationReference {
    pub policy: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PolicyKind {
    ProviderToken,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderPolicy {
    pub kind: PolicyKind,
    pub project_keys: Vec<String>,
    pub allow_account_binding: bool,
    pub allow_project_sub_token: bool,
    pub api_types: ApiTypes,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum ApiTypes {
    NotApplicable,
    AnyOf {
        values: Vec<String>,
        empty_list: EmptyList,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EmptyList {
    Allow,
    Deny,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResolvedApiTypes {
    NotApplicable,
    AnyOf {
        values: Vec<i64>,
        empty_list: EmptyList,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ResolvedProviderPolicy {
    pub kind: PolicyKind,
    pub project_keys: Vec<String>,
    pub projects: BTreeMap<String, ProjectDefinition>,
    pub allow_account_binding: bool,
    pub allow_project_sub_token: bool,
    pub api_types: ResolvedApiTypes,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RouteAuthorization {
    pub schema_version: u32,
    pub canonical_path: String,
    pub policy_id: String,
    pub policy_hash: String,
    pub required_capabilities: Vec<String>,
    pub resolved_policy: ResolvedProviderPolicy,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProtectedRoute {
    pub route_id: String,
    pub canonical_path: String,
    pub method: String,
    pub authorization: RouteAuthorization,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RoutePolicyBundle {
    pub schema_version: u32,
    pub enforced_route_prefixes: Vec<String>,
    pub enforced_source_groups: Vec<String>,
    pub projects: BTreeMap<String, ProjectDefinition>,
    pub routes: Vec<ProtectedRoute>,
    pub bundle_hash: String,
}

fn identifier(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
}
fn segments(s: &str) -> bool {
    s.split('/').all(identifier)
}
fn sorted_unique<T: Ord + Clone>(items: &[T], label: &str) -> Result<Vec<T>> {
    let sorted: BTreeSet<_> = items.iter().cloned().collect();
    ensure!(sorted.len() == items.len(), "duplicate {label}");
    Ok(sorted.into_iter().collect())
}

/// Canonical object ordering makes hashes independent of configuration key order.
pub fn semantic_hash(value: &impl Serialize) -> Result<String> {
    fn canonical(value: Value) -> Value {
        match value {
            Value::Object(items) => Value::Object(
                items
                    .into_iter()
                    .map(|(k, v)| (k, canonical(v)))
                    .collect::<BTreeMap<_, _>>()
                    .into_iter()
                    .collect(),
            ),
            Value::Array(items) => Value::Array(items.into_iter().map(canonical).collect()),
            value => value,
        }
    }
    Ok(crate::incremental::digest(&serde_json::to_vec(
        &canonical(serde_json::to_value(value)?),
    )?))
}

fn resolve_api_types(
    value: &ApiTypes,
    source: &Path,
    catalog_name: Option<&str>,
) -> Result<ResolvedApiTypes> {
    match value {
        ApiTypes::NotApplicable => Ok(ResolvedApiTypes::NotApplicable),
        ApiTypes::AnyOf { values, empty_list } => {
            ensure!(
                !values.is_empty(),
                "api_types.any_of values must not be empty"
            );
            let values = sorted_unique(values, "api_types symbol")?;
            let path = source
                .parent()
                .unwrap()
                .join(catalog_name.unwrap_or("enum_catalog.json"));
            let catalog: Value = serde_json::from_slice(
                &fs::read(&path)
                    .with_context(|| format!("read API type catalog {}", path.display()))?,
            )?;
            ensure!(catalog["version"] == 1, "unsupported enum_catalog version");
            let rows = catalog["enums"]["api_type"]["field_list"]
                .as_array()
                .context("api_type enum catalog missing")?;
            let mut result = Vec::new();
            for name in values {
                let matches: Vec<_> = rows
                    .iter()
                    .filter(|row| row["field_name"].as_str() == Some(&name))
                    .collect();
                ensure!(
                    matches.len() == 1,
                    "unknown or duplicate api_type symbol {name}"
                );
                let value = matches[0]["field_value"]
                    .as_i64()
                    .context("api_type value must be an integer")?;
                ensure!(value > 0, "api_type value must be positive");
                result.push(value);
            }
            let result = sorted_unique(&result, "api_type value")?;
            Ok(ResolvedApiTypes::AnyOf {
                values: result,
                empty_list: empty_list.clone(),
            })
        }
    }
}

pub fn compile(
    config: Option<&AuthorizationConfig>,
    endpoints: &mut [Endpoint],
    source: &Path,
    catalog_name: Option<&str>,
) -> Result<Option<RoutePolicyBundle>> {
    let Some(config) = config else {
        ensure!(
            endpoints.iter().all(|a| a.meta.authorization.is_none()),
            "authorization reference requires config.json authorization configuration"
        );
        return Ok(None);
    };
    ensure!(
        config.schema_version == SCHEMA_VERSION,
        "unsupported authorization schema_version"
    );
    ensure!(
        !config.projects.is_empty() && !config.policies.is_empty(),
        "authorization projects and policies must not be empty"
    );
    let prefixes = sorted_unique(&config.enforced_route_prefixes, "enforced route prefix")?;
    for p in &prefixes {
        ensure!(
            p.starts_with('/') && p.ends_with('/') && p.len() > 2 && segments(&p[1..p.len() - 1]),
            "invalid enforced route prefix {p}"
        );
    }
    let groups = sorted_unique(&config.enforced_source_groups, "enforced source group")?;
    for g in &groups {
        ensure!(segments(g), "invalid enforced source group {g}");
    }
    let mut tenants = BTreeSet::new();
    for (key, project) in &config.projects {
        ensure!(
            identifier(key) && identifier(&project.project_type) && project.tenant_id > 0,
            "invalid authorization project {key}"
        );
        ensure!(
            tenants.insert(project.tenant_id),
            "duplicate project tenant_id {}",
            project.tenant_id
        );
    }
    let mut policies = BTreeMap::new();
    for (id, policy) in &config.policies {
        ensure!(identifier(id), "invalid policy_id {id}");
        ensure!(
            !policy.project_keys.is_empty(),
            "policy {id}: project_keys must not be empty"
        );
        let keys = sorted_unique(&policy.project_keys, "policy project key")?;
        let mut projects = BTreeMap::new();
        for key in &keys {
            projects.insert(
                key.clone(),
                config
                    .projects
                    .get(key)
                    .with_context(|| format!("policy {id}: unknown project {key}"))?
                    .clone(),
            );
        }
        let resolved = ResolvedProviderPolicy {
            kind: policy.kind.clone(),
            project_keys: keys,
            projects,
            allow_account_binding: policy.allow_account_binding,
            allow_project_sub_token: policy.allow_project_sub_token,
            api_types: resolve_api_types(&policy.api_types, source, catalog_name)?,
        };
        let mut capabilities = vec!["provider_token.v1".to_owned()];
        if policy.allow_account_binding {
            capabilities.push("account_binding.v1".into());
        }
        if policy.allow_project_sub_token {
            capabilities.push("project_sub_token.v1".into());
        }
        if matches!(policy.api_types, ApiTypes::AnyOf { .. }) {
            capabilities.push("api_types.v1".into());
        }
        capabilities.sort();
        let hash = semantic_hash(
            &json!({"schema_version":SCHEMA_VERSION,"resolved_policy":resolved,"required_capabilities":capabilities}),
        )?;
        policies.insert(id.clone(), (resolved, capabilities, hash));
    }
    for group in &groups {
        ensure!(
            endpoints
                .iter()
                .any(|a| a.group == *group || a.group.starts_with(&format!("{group}/"))),
            "enforced source group {group} matches no routes"
        );
    }
    let mut routes = Vec::new();
    for endpoint in endpoints {
        let enforced = prefixes.iter().any(|p| endpoint.path.starts_with(p))
            || groups
                .iter()
                .any(|g| endpoint.group == *g || endpoint.group.starts_with(&format!("{g}/")));
        let Some(reference) = &endpoint.meta.authorization else {
            ensure!(
                !enforced,
                "{}: missing authorization policy in enforced scope",
                endpoint.source
            );
            continue;
        };
        let (resolved, capabilities, hash) =
            policies.get(&reference.policy).with_context(|| {
                format!(
                    "{}: unknown authorization policy {}",
                    endpoint.source, reference.policy
                )
            })?;
        ensure!(
            endpoint.meta.auth == "SESSION",
            "{}: provider_token policy requires SESSION auth",
            endpoint.source
        );
        let authorization = RouteAuthorization {
            schema_version: SCHEMA_VERSION,
            canonical_path: endpoint.path.clone(),
            policy_id: reference.policy.clone(),
            policy_hash: hash.clone(),
            required_capabilities: capabilities.clone(),
            resolved_policy: resolved.clone(),
        };
        routes.push(ProtectedRoute {
            route_id: endpoint.name(),
            canonical_path: endpoint.path.clone(),
            method: if endpoint.method().is_empty() {
                "POST".into()
            } else {
                endpoint.method().into()
            },
            authorization: authorization.clone(),
        });
        endpoint.authorization = Some(authorization);
    }
    routes.sort_by(|a, b| a.route_id.cmp(&b.route_id));
    let mut bundle = RoutePolicyBundle {
        schema_version: SCHEMA_VERSION,
        enforced_route_prefixes: prefixes,
        enforced_source_groups: groups,
        projects: config.projects.clone(),
        routes,
        bundle_hash: String::new(),
    };
    let mut value = serde_json::to_value(&bundle)?;
    value.as_object_mut().unwrap().remove("bundle_hash");
    bundle.bundle_hash = semantic_hash(&value)?;
    Ok(Some(bundle))
}
