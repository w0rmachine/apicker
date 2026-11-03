use serde_json::Value;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::sleep;
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_apicker");
static NEXT_ID: AtomicU64 = AtomicU64::new(0);

struct TestEnv {
    root: PathBuf,
    home: PathBuf,
    config_home: PathBuf,
    cache_home: PathBuf,
    data_home: PathBuf,
    system_data: PathBuf,
    bin: PathBuf,
}

impl TestEnv {
    fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "apicker-cli-{}-{}-{label}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        let env = Self {
            home: root.join("home"),
            config_home: root.join("config"),
            cache_home: root.join("cache"),
            data_home: root.join("data"),
            system_data: root.join("system-data"),
            bin: root.join("bin"),
            root,
        };
        for dir in [
            &env.home,
            &env.config_home,
            &env.cache_home,
            &env.data_home,
            &env.system_data,
            &env.bin,
        ] {
            fs::create_dir_all(dir).unwrap();
        }
        env
    }

    fn config(&self, body: &str) {
        self.write(
            self.config_home.join("apicker/config.toml"),
            body.as_bytes(),
        );
    }

    fn write(&self, path: PathBuf, bytes: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn executable(&self, name: &str, body: &str) -> PathBuf {
        let path = self.bin.join(name);
        self.write(path.clone(), body.as_bytes());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn desktop(&self, dir: &Path, name: &str, body: &str) {
        self.write(dir.join("applications").join(name), body.as_bytes());
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(BIN)
            .args(args)
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", &self.config_home)
            .env("XDG_CACHE_HOME", &self.cache_home)
            .env("XDG_DATA_HOME", &self.data_home)
            .env("XDG_DATA_DIRS", &self.system_data)
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    self.bin.display(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            )
            .output()
            .unwrap()
    }

    fn search(&self, args: &[&str]) -> Value {
        let mut full_args = vec!["search"];
        full_args.extend_from_slice(args);
        let out = self.run(&full_args);
        assert_success(&out);
        serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
            panic!(
                "invalid JSON: {e}\nstdout: {}\nstderr: {}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            )
        })
    }

    fn cache_file(&self) -> PathBuf {
        self.cache_home.join("apicker/paths.txt")
    }

    fn mru_file(&self) -> PathBuf {
        self.cache_home.join("apicker/mru.json")
    }

    fn rebuild(&self, root: &Path) -> Output {
        self.run(&[
            "rebuild-cache",
            "--root",
            root.to_str().expect("test path is UTF-8"),
            "--no-default-ignores",
        ])
    }
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn assert_success(out: &Output) {
    assert!(
        out.status.success(),
        "command failed: {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

fn wait_for(path: &Path) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Ok(content) = fs::read_to_string(path) {
            return content;
        }
        sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for {}", path.display());
}

fn toml_string(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

fn source_config(name: &str, script: &str, action: &str) -> String {
    format!(
        r#"
[[sources]]
name = "{name}"
icon = "test-icon"
description = "Test source"
script = "{}"
action = "{}"
"#,
        toml_string(script),
        toml_string(action)
    )
}

#[test]
fn list_scripts_outputs_configured_sources() {
    let env = TestEnv::new("list-scripts");
    env.config(&source_config("power", "printf 'Shutdown\\n'", "true"));

    let out = env.run(&["list-scripts"]);
    assert_success(&out);
    let json: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json[0]["name"], "power");
    assert_eq!(json[0]["icon"], "test-icon");
    assert_eq!(json[0]["script"], "printf 'Shutdown\\n'");
}
#[test]
fn search_limit_is_applied_after_sorting() {
    let env = TestEnv::new("limit");
    let config = format!(
        "{}{}{}",
        source_config("first", "printf 'one\\n'", "true"),
        source_config("second", "printf 'two\\n'", "true"),
        source_config("third", "printf 'three\\n'", "true")
    );
    env.config(&config);

    let json = env.search(&["--query", ":", "--limit", "2"]);
    let items = json.as_array().unwrap();
    assert_eq!(items.len(), 2);
    for item in items {
        assert_eq!(item["is_submenu"], true);
    }
}
#[test]
fn malformed_config_falls_back_to_empty_sources() {
    let env = TestEnv::new("bad-config");
    env.config("this is not valid toml [");

    let out = env.run(&["list-scripts"]);
    assert_success(&out);
    let json: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json, Value::Array(Vec::new()));
    assert!(String::from_utf8_lossy(&out.stderr).contains("Error parsing config.toml"));
}

#[test]
fn custom_source_search_filters_and_mru_boost_is_visible() {
    let env = TestEnv::new("source-search");
    env.config(&source_config(
        "power",
        "printf 'Shutdown\\nReboot\\nLogout\\n'",
        "true",
    ));

    let out = env.run(&["execute", "--source", "power", "--item", "Shutdown"]);
    assert_success(&out);

    let json = env.search(&["--source", "power", "--query", "sh"]);
    let items = json.as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["text"], "Shutdown");
    assert!(items[0]["score"].as_i64().unwrap() >= 50);

    let json = env.search(&["--source", "power", "--query", ""]);
    assert_eq!(json[0]["text"], "Shutdown");
}

#[test]
fn custom_action_receives_the_item_as_one_escaped_argument() {
    let env = TestEnv::new("custom-action");
    let marker = env.root.join("action-marker");
    let config = format!(
        r#"
[[sources]]
name = "tricky"
icon = "test-icon"
script = "printf \"Alice's PC\\n\""
action = 'printf "%s" "{{item}}" > {}'
"#,
        marker.display()
    );
    env.config(&config);

    let json = env.search(&["--source", "tricky", "--query", "alice"]);
    let item = json[0]["path"].as_str().unwrap().to_string();
    assert_eq!(item, "Alice's PC");

    let out = env.run(&["execute", "--source", "tricky", "--item", &item]);
    assert_success(&out);
    assert_eq!(wait_for(&marker), item);

    let mru: Value = serde_json::from_slice(&fs::read(env.mru_file()).unwrap()).unwrap();
    assert_eq!(mru["counts"][&item], 1);
}

#[test]
fn default_execute_opens_the_item_with_xdg_open() {
    let env = TestEnv::new("xdg-open");
    let marker = env.root.join("xdg-marker");
    env.executable(
        "xdg-open",
        &format!("#!/bin/sh\nprintf '%s' \"$1\" > '{}'\n", marker.display()),
    );

    let item = env.root.join("target file.txt");
    let out = env.run(&["execute", "--item", item.to_str().unwrap()]);
    assert_success(&out);
    assert_eq!(wait_for(&marker), item.to_str().unwrap());

    let mru: Value = serde_json::from_slice(&fs::read(env.mru_file()).unwrap()).unwrap();
    assert_eq!(mru["counts"][item.to_str().unwrap()], 1);
}
