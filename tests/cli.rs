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

    fn rebuild_hidden(&self, root: &Path) -> Output {
        self.run(&[
            "rebuild-cache",
            "--root",
            root.to_str().expect("test path is UTF-8"),
            "--hidden",
        ])
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
    let tmp = marker.with_extension("tmp");
    // Write-then-rename so wait_for never reads a half-written file.
    env.executable(
        "xdg-open",
        &format!(
            "#!/bin/sh\nprintf '%s' \"$1\" > '{}'; mv '{}' '{}'\n",
            tmp.display(),
            tmp.display(),
            marker.display()
        ),
    );

    let item = env.root.join("target file.txt");
    let out = env.run(&["execute", "--item", item.to_str().unwrap()]);
    assert_success(&out);
    assert_eq!(wait_for(&marker), item.to_str().unwrap());

    let mru: Value = serde_json::from_slice(&fs::read(env.mru_file()).unwrap()).unwrap();
    assert_eq!(mru["counts"][item.to_str().unwrap()], 1);
}

#[test]
fn files_cache_honors_ignores_and_search_uses_cached_kind_icons() {
    let env = TestEnv::new("files");
    env.config("ignore_dirs = [\"node_modules\"]\n");

    let scan = env.root.join("scan");
    let scan_str = scan.to_str().expect("test path is UTF-8");
    env.write(scan.join("alpha.txt"), b"a");
    env.write(scan.join("project/beta.rs"), b"b");
    env.write(scan.join("project/node_modules/ignored.js"), b"c");
    env.write(scan.join("node_modules/ignored.rs"), b"d");
    env.write(scan.join(".hidden/secret.txt"), b"e");
    env.write(scan.join(".dotfile"), b"f");

    let out = env.rebuild(&scan);
    assert_success(&out);
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("4 items"),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );

    let cache = fs::read_to_string(env.cache_file()).unwrap();
    assert!(cache.contains("alpha.txt"));
    assert!(cache.contains("beta.rs"));
    assert!(!cache.contains("node_modules"));
    assert!(!cache.contains(".hidden"));
    assert!(!cache.contains(".dotfile"));

    let json = env.search(&["--source", "files", "--root", scan_str, "--query", "beta"]);
    let items = json.as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["text"], "beta.rs");
    assert_eq!(items[0]["icon"], "code-context");
    assert_eq!(items[0]["is_dir"], false);
    assert!(items[0]["path"]
        .as_str()
        .unwrap()
        .ends_with("project/beta.rs"));

    let json = env.search(&[
        "--source", "files", "--root", scan_str, "--query", "project",
    ]);
    let project = json
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["path"].as_str().unwrap().ends_with("/project"))
        .expect("project directory should be searchable");
    assert_eq!(project["icon"], "folder");
    assert_eq!(project["is_dir"], true);
}

#[test]
fn files_search_can_index_hidden_entries() {
    let env = TestEnv::new("hidden-search");
    let scan = env.root.join("scan");
    let scan_str = scan.to_str().expect("test path is UTF-8");
    env.write(scan.join(".config/app.ini"), b"x");

    let out = env.rebuild_hidden(&scan);
    assert_success(&out);
    let cache = fs::read_to_string(env.cache_file()).unwrap();
    assert!(cache.contains(".config/app.ini"), "{cache}");

    let json = env.search(&[
        "--source", "files", "--root", scan_str, "--query", "app.ini", "--hidden",
    ]);
    let items = json.as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["text"], "app.ini");
}

#[test]
fn desktop_search_skips_hidden_entries_and_launches_by_path() {
    let env = TestEnv::new("desktop");
    let marker = env.root.join("desktop-marker");
    let app = env.executable(
        "test-app",
        &format!("#!/bin/sh\nprintf 'launched' > '{}'\n", marker.display()),
    );

    env.desktop(
        &env.data_home,
        "test.desktop",
        &format!(
            "[Desktop Entry]\nType=Application\nName=Test App\nExec={} %U\nIcon=test-icon\n",
            app.display()
        ),
    );
    env.desktop(
        &env.data_home,
        "hidden.desktop",
        &format!(
            "[Desktop Entry]\nType=Application\nName=Hidden App\nExec={}\nNoDisplay=true\n",
            app.display()
        ),
    );

    let json = env.search(&["--source", "desktop", "--query", "test"]);
    let items = json.as_array().unwrap();
    assert_eq!(items.len(), 1, "unexpected desktop results: {items:?}");
    assert_eq!(items[0]["text"], "Test App");
    assert_eq!(items[0]["icon"], "test-icon");
    let path = items[0]["path"].as_str().unwrap().to_string();
    assert!(path.ends_with("/applications/test.desktop"), "{path}");

    assert!(env
        .search(&["--source", "desktop", "--query", "hidden"])
        .as_array()
        .unwrap()
        .is_empty());

    let out = env.run(&["execute", "--source", "desktop", "--item", &path]);
    assert_success(&out);
    assert_eq!(wait_for(&marker), "launched");

    let mru: Value = serde_json::from_slice(&fs::read(env.mru_file()).unwrap()).unwrap();
    let counts = mru["counts"].as_object().unwrap();
    assert!(
        counts
            .keys()
            .any(|key| key.starts_with(app.to_str().unwrap())),
        "desktop MRU key should be the resolved argv: {counts:?}"
    );
}

#[test]
fn colon_mode_lists_only_sources() {
    let env = TestEnv::new("colon");
    env.config(&source_config("power", "printf 'Shutdown\\n'", "true"));

    let scan = env.root.join("scan");
    env.write(scan.join("note.txt"), b"hello");
    assert_success(&env.rebuild(&scan));

    let app = env.executable("fake-app", "#!/bin/sh\nexit 0\n");
    env.desktop(
        &env.data_home,
        "test.desktop",
        &format!(
            "[Desktop Entry]\nType=Application\nName=Test App\nExec={}\nIcon=app\n",
            app.display()
        ),
    );

    let sources = env.search(&["--query", ":"]);
    let sources = sources.as_array().unwrap();
    assert_eq!(
        sources.len(),
        1,
        "colon mode leaked built-in results: {sources:?}"
    );
    assert_eq!(sources[0]["is_submenu"], true);
    assert_eq!(sources[0]["target_source"], "power");

    let filtered = env.search(&["--query", ":pow"]);
    assert_eq!(filtered.as_array().unwrap().len(), 1);
    assert_eq!(filtered[0]["path"], "power");

    let none = env.search(&["--query", ":zzz"]);
    assert!(none.as_array().unwrap().is_empty(), "{none}");
}

#[test]
fn empty_query_lists_sources_and_apps() {
    let env = TestEnv::new("empty-query");
    env.config(&source_config("power", "printf 'Shutdown\\n'", "true"));

    let scan = env.root.join("scan");
    env.write(scan.join("note.txt"), b"hello");
    assert_success(&env.rebuild(&scan));

    let app = env.executable("fake-app", "#!/bin/sh\nexit 0\n");
    env.desktop(
        &env.data_home,
        "test.desktop",
        &format!(
            "[Desktop Entry]\nType=Application\nName=Test App\nExec={}\nIcon=app\n",
            app.display()
        ),
    );

    // An empty query is the recent-files view upstream: files with no MRU
    // history are dropped, so only sources and apps show up here.
    let all = env.search(&["--query", ""]);
    let all = all.as_array().unwrap();
    assert!(
        all.iter().any(|item| item["target_source"] == "power"),
        "{all:?}"
    );
    assert!(all.iter().any(|item| item["text"] == "Test App"), "{all:?}");
}
