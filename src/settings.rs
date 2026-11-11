use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::{fs, path::Path};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Settings {
    #[serde(default)]
    pub project_list: Vec<Project>,
    pub enum_catalog: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authorization: Option<crate::authorization::AuthorizationConfig>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    pub apifox_project_name: String,
    #[serde(default)]
    pub apifox_token: String,
    pub apifox_project_id: Option<Value>,
    pub test_apifox_project_id: Option<Value>,
    #[serde(default)]
    pub url_replace_cfg: Map<String, Value>,
    #[serde(default)]
    pub tag_list: Vec<String>,
    pub export_tags_flag: Option<bool>,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub import_options: ImportOptions,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ImportOptions {
    pub target_endpoint_folder_id: Option<u64>,
    pub target_schema_folder_id: Option<u64>,
    pub target_branch_id: Option<u64>,
    pub module_id: Option<u64>,
    #[serde(default)]
    pub endpoint_overwrite: Overwrite,
    #[serde(default)]
    pub schema_overwrite: Overwrite,
    #[serde(default)]
    pub update_folder: bool,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Overwrite {
    #[default]
    OverwriteExisting,
    AutoMerge,
    KeepExisting,
}
impl ImportOptions {
    pub fn payload(&self) -> Value {
        let mut v = json!({"endpointOverwriteBehavior":self.endpoint_overwrite,"schemaOverwriteBehavior":self.schema_overwrite,"updateFolderOfChangedEndpoint":self.update_folder,"prependBasePath":false,"deleteUnmatchedResources":false});
        for (key, value) in [
            ("targetEndpointFolderId", self.target_endpoint_folder_id),
            ("targetSchemaFolderId", self.target_schema_folder_id),
            ("targetBranchId", self.target_branch_id),
            ("moduleId", self.module_id),
        ] {
            if let Some(n) = value {
                v[key] = json!(n);
            }
        }
        v
    }
}
impl Settings {
    pub fn load(proto_dir: &Path) -> Result<Self> {
        let file = proto_dir.join("config.json");
        if !file.exists() {
            ensure!(
                !proto_dir.join("config.php").exists(),
                "migrate proto/config.php to proto/config.json"
            );
            return Ok(Self::default());
        }
        let value: Self = serde_json::from_slice(&fs::read(&file)?)
            .with_context(|| format!("invalid project configuration: {}", file.display()))?;
        ensure!(
            value.extra.keys().all(|k| k == "new_control_flag"),
            "unknown root config.json setting"
        );
        if let Some(v) = value.extra.get("new_control_flag") {
            ensure!(v.is_boolean(), "new_control_flag must be boolean");
        }
        Ok(value)
    }
    // Keep the published PHP metadata contract while using the shared JSON source.
    pub fn php_metadata(&self) -> Value {
        let mut root = self.extra.clone();
        let mut exports = Map::new();
        for p in &self.project_list {
            let mut item = serde_json::to_value(p).expect("project is serializable");
            let object = item.as_object_mut().unwrap();
            object.remove("name");
            object.remove("import_options");
            object.retain(|_, v| !v.is_null());
            if p.url_replace_cfg.is_empty() {
                object.insert("url_replace_cfg".into(), json!([]));
            }
            exports.insert(
                if p.name.is_empty() {
                    p.apifox_project_name.clone()
                } else {
                    p.name.clone()
                },
                item,
            );
        }
        root.entry("new_control_flag").or_insert(json!(true));
        root.insert(
            "export".into(),
            if exports.is_empty() {
                json!([])
            } else {
                json!(exports)
            },
        );
        json!(root)
    }
}
