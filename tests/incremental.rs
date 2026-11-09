//! Exercise the public CLI with a real compiler and a per-project invocation log.
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn put(root: &Path, path: &str, text: &str) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}
fn quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}
struct Project {
    dir: tempfile::TempDir,
}
impl Project {
    fn new(target: &str) -> Self {
        let project = Self {
            dir: tempfile::tempdir().unwrap(),
        };
        let root = project.root();
        put(
            root,
            "proto/src/common/api_options.proto",
            workspace_proto_gen::compiler::SCHEMA,
        );
        put(
            root,
            "proto/src/common/base.proto",
            "syntax=\"proto3\"; package common; message Base { string name=1; }\n",
        );
        put(root, "proto/src/common/shared.proto", "syntax=\"proto3\"; package common; import \"common/base.proto\"; message Shared { Base value=1; }\n");
        for (action, field) in [
            ("first", "common.Shared value=1;"),
            ("second", "string value=1;"),
        ] {
            put(root, &format!("proto/src/root/test__{action}.proto"), &format!("syntax=\"proto3\";\nimport \"common/api_options.proto\";\n{}\noption(common.api)={{title:\"{action}\" auth:PUBLIC description_file:\"description.md\"}};\nmessage in {{ {field} }}\nmessage out {{ string result=1; }}\n", if action == "first" { "import \"common/shared.proto\";" } else { "" }));
        }
        put(root, "proto/src/root/description.md", "Initial description");
        put(root, "proto/generator.toml", &format!("version=1\ntarget=\"{target}\"\nsource=\"src\"\noutput=\"../gen/proto\"\ncontrollers=\"../controllers\"\nregistry=\"../gen/registry.rs\"\n"));
        let protoc = workspace_proto_gen::compiler::protoc();
        let protoc = if protoc.is_absolute() {
            protoc
        } else {
            std::env::split_paths(&std::env::var_os("PATH").unwrap())
                .map(|p| p.join(&protoc))
                .find(|p| p.is_file())
                .unwrap()
        };
        put(
            root,
            "protoc",
            &format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> {}\nexec {} \"$@\"\n",
                quote(&root.join("calls")),
                quote(&protoc)
            ),
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(root.join("protoc"), fs::Permissions::from_mode(0o755)).unwrap();
        }
        project
    }
    fn root(&self) -> &Path {
        self.dir.path()
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_proto-gen"))
            .args(["generate", "--config"])
            .arg(self.root().join("proto/generator.toml"))
            .args(args)
            .env("PROTOC", self.root().join("protoc"))
            .output()
            .unwrap()
    }
    fn ok(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }
    fn calls(&self) -> String {
        fs::read_to_string(self.root().join("calls")).unwrap_or_default()
    }
    fn reset_calls(&self) {
        fs::write(self.root().join("calls"), "").unwrap();
    }
    fn replace(&self, path: &str, from: &str, to: &str) {
        let file = self.root().join(path);
        fs::write(&file, fs::read_to_string(&file).unwrap().replace(from, to)).unwrap();
    }
    fn artifacts(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        let names: Vec<String> = serde_json::from_slice(
            &fs::read(self.root().join("proto/.proto-gen-manifest.json")).unwrap(),
        )
        .unwrap();
        names
            .into_iter()
            .map(|name| {
                let bytes = fs::read(self.root().join(&name)).unwrap();
                (PathBuf::from(name), bytes)
            })
            .collect()
    }
    fn equivalent(&self) {
        let incremental = self.artifacts();
        let state = fs::read(self.root().join("proto/.proto-gen-cache.json")).unwrap();
        self.ok(&["--check"]);
        assert_eq!(
            state,
            fs::read(self.root().join("proto/.proto-gen-cache.json")).unwrap(),
            "--check must not update state"
        );
        self.ok(&["--force"]);
        assert_eq!(
            incremental,
            self.artifacts(),
            "incremental and forced full generation differ"
        );
    }
}
#[test]
fn unchanged_metadata_dependencies_deletion_and_repair() {
    let project = Project::new("php");
    project.ok(&[]);
    project.reset_calls();
    assert!(project.ok(&[]).contains("compilation skipped"));
    assert!(project.calls().is_empty());
    project.replace(
        "proto/src/root/test__first.proto",
        "title:\"first\"",
        "title:\"edited\"",
    );
    assert!(project
        .ok(&[])
        .contains("0 binding units compiled, 4 reused"));
    assert!(!project.calls().contains("--php_out"));
    project.equivalent();
    project.reset_calls();
    project.replace(
        "proto/src/common/base.proto",
        "string name=1;",
        "string name=1; optional int32 age=2;",
    );
    let out = project.ok(&[]);
    assert!(out.contains("3 binding units compiled, 1 reused"), "{out}");
    let calls = project.calls();
    let bindings: Vec<_> = calls.lines().filter(|l| l.contains("--php_out")).collect();
    assert_eq!(bindings.len(), 3);
    assert!(bindings
        .iter()
        .all(|line| !line.contains("test__second.proto")));
    project.equivalent();
    let controller = project.root().join("controllers/Root/Test.php");
    let handwritten = fs::read_to_string(&controller).unwrap().replace(
        "return $this->output_err",
        "/* custom implementation */ return $this->output_err",
    );
    fs::write(&controller, &handwritten).unwrap();
    project.ok(&[]);
    assert_eq!(fs::read_to_string(&controller).unwrap(), handwritten);
    fs::remove_file(project.root().join("proto/src/root/test__first.proto")).unwrap();
    assert!(project
        .ok(&[])
        .contains("0 binding units compiled, 3 reused"));
    assert!(!project
        .artifacts()
        .keys()
        .any(|p| p.to_string_lossy().contains("test__first")));
    assert_eq!(fs::read_to_string(&controller).unwrap(), handwritten);
    project.equivalent();
    let original = project.artifacts();
    let binding = original
        .keys()
        .find(|p| p.to_string_lossy().ends_with("test__second/in.php"))
        .unwrap();
    fs::write(project.root().join(binding), "damaged").unwrap();
    assert!(!project.run(&["--check"]).status.success());
    assert!(project
        .ok(&[])
        .contains("1 binding units compiled, 2 reused"));
    assert_eq!(original, project.artifacts());
    fs::remove_file(&controller).unwrap();
    project.ok(&[]);
    assert!(fs::read_to_string(&controller)
        .unwrap()
        .contains("public function second("));
}
#[test]
fn description_settings_cache_corruption_and_failure_do_not_stale_or_advance_state() {
    let project = Project::new("php");
    project.ok(&[]);
    project.reset_calls();
    put(
        project.root(),
        "proto/src/root/description.md",
        "Revised description",
    );
    assert!(project
        .ok(&[])
        .contains("0 binding units compiled, 4 reused"));
    assert!(
        fs::read_to_string(project.root().join("gen/proto/cmd.json"))
            .unwrap()
            .contains("Revised description")
    );
    put(
        project.root(),
        "proto/config.json",
        "{\"new_control_flag\":false}",
    );
    assert!(project
        .ok(&[])
        .contains("0 binding units compiled, 4 reused"));
    assert!(!project.calls().contains("--php_out"));
    project.equivalent();
    let state_path = project.root().join("proto/.proto-gen-cache.json");
    let state = fs::read(&state_path).unwrap();
    let artifacts = project.artifacts();
    project.replace(
        "proto/src/root/test__second.proto",
        "title:\"second\"",
        "title:\"invalid\" http:{path:\"/root/test/first\"}",
    );
    assert!(!project.run(&[]).status.success());
    assert_eq!(state, fs::read(&state_path).unwrap());
    assert_eq!(artifacts, project.artifacts());
    project.replace(
        "proto/src/root/test__second.proto",
        "title:\"invalid\" http:{path:\"/root/test/first\"}",
        "title:\"second\"",
    );
    fs::write(&state_path, "invalid json").unwrap();
    assert!(project
        .ok(&[])
        .contains("4 binding units compiled, 0 reused"));
    project.equivalent();
}
#[test]
fn rust_metadata_reuses_bindings_and_forced_output_is_identical() {
    let project = Project::new("rust");
    project.ok(&[]);
    project.reset_calls();
    assert!(project.ok(&[]).contains("compilation skipped"));
    assert!(project.calls().is_empty());
    project.replace(
        "proto/src/root/test__first.proto",
        "title:\"first\"",
        "title:\"edited\"",
    );
    assert!(project
        .ok(&[])
        .contains("0 binding units compiled, 1 reused"));
    project.equivalent();
    project.replace(
        "proto/src/common/base.proto",
        "string name=1;",
        "string name=1; optional int32 age=2;",
    );
    assert!(project
        .ok(&[])
        .contains("1 binding units compiled, 0 reused"));
    project.equivalent();
}
#[test]
fn tool_changes_invalidate_bindings_and_failed_install_rolls_back_manifest() {
    let project = Project::new("php");
    project.ok(&[]);
    let tool = project.root().join("protoc");
    let text = fs::read_to_string(&tool).unwrap();
    fs::write(&tool, format!("{text}\n# tool revision\n")).unwrap();
    assert!(project
        .ok(&[])
        .contains("4 binding units compiled, 0 reused"));
    let artifacts = project.artifacts();
    let manifest = fs::read(project.root().join("proto/.proto-gen-manifest.json")).unwrap();
    let state_path = project.root().join("proto/.proto-gen-cache.json");
    let state = fs::read(&state_path).unwrap();
    fs::remove_file(&state_path).unwrap();
    fs::create_dir(&state_path).unwrap();
    project.replace(
        "proto/src/root/test__second.proto",
        "string value=1;",
        "string value=1; optional bool added=2;",
    );
    let result = project.run(&[]);
    assert!(!result.status.success());
    assert_eq!(artifacts, project.artifacts());
    assert_eq!(
        manifest,
        fs::read(project.root().join("proto/.proto-gen-manifest.json")).unwrap()
    );
    assert!(state_path.is_dir());
    fs::remove_dir(&state_path).unwrap();
    fs::write(&state_path, state).unwrap();
    project.ok(&[]);
    project.equivalent();
}

// Optional real-plugin coverage. CI/release environments can provide these paths;
// tests never install packages into a business repository.
#[test]
fn typescript_real_plugin_incremental_matches_batch_generation() {
    let Some(modules) = std::env::var_os("PROTO_GEN_TEST_NODE_MODULES") else {
        return;
    };
    let project = Project::new("typescript");
    #[cfg(unix)]
    std::os::unix::fs::symlink(modules, project.root().join("node_modules")).unwrap();
    real_plugin_edits(&project, 1, 1);
}
#[test]
fn go_real_plugin_incremental_matches_batch_generation() {
    let Some(plugin) = std::env::var_os("PROTO_GEN_TEST_GO_PLUGIN") else {
        return;
    };
    let project = Project::new("go");
    put(
        project.root(),
        "go.mod",
        "module example.test/project\n\ngo 1.24\n",
    );
    put(project.root(), "proto/generator.toml", "version=1\ntarget=\"go\"\nsource=\"src\"\noutput=\"../gen/proto\"\ncontrollers=\"../controllers\"\nregistry=\"../gen/registry.go\"\n");
    fs::create_dir_all(project.root().join(".tools")).unwrap();
    fs::copy(plugin, project.root().join(".tools/protoc-gen-go")).unwrap();
    real_plugin_edits(&project, 3, 1);
}
fn real_plugin_edits(project: &Project, compiled: usize, reused: usize) {
    project.ok(&[]);
    project.equivalent();
    project.reset_calls();
    assert!(project.ok(&[]).contains("compilation skipped"));
    assert!(project.calls().is_empty());
    project.replace(
        "proto/src/root/test__first.proto",
        "title:\"first\"",
        "title:\"edited\"",
    );
    assert!(project.ok(&[]).contains("0 binding units compiled"));
    project.equivalent();
    project.replace(
        "proto/src/common/base.proto",
        "string name=1;",
        "string name=1; optional int32 age=2;",
    );
    let out = project.ok(&[]);
    assert!(
        out.contains(&format!(
            "{compiled} binding units compiled, {reused} reused"
        )),
        "{out}"
    );
    project.equivalent();
    fs::remove_file(project.root().join("proto/src/root/test__first.proto")).unwrap();
    project.ok(&[]);
    project.equivalent();
}
