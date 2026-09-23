mod core;
mod mru;
mod provider;

use crate::core::PickerItem;
use crate::mru::MruCache;
use crate::provider::{
    desktop::{self, DesktopProvider},
    desktop_spec,
    files::rebuild_cache,
    files::FilesProvider,
    Provider,
};

use clap::{Parser, Subcommand};
use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;

/// MRU boost multiplier — each recent-use count adds this much to the score.
pub const MRU_BOOST_FACTOR: i64 = 50;

/// Base score for source submenu entries so they rank above regular results.
const SOURCE_BASE_SCORE: i64 = 1000;

/// Bonus range for shorter source names (shorter = higher priority).
const SOURCE_NAME_LENGTH_BASE: i64 = 10;

#[derive(Parser, Debug)]
#[command(name = "apicker")]
#[command(about = "A fast, scriptable file picker backend in Rust", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Search files/entries using cache and filter
    Search {
        /// The search query
        #[arg(short, long, default_value = "")]
        query: String,

        /// Filter by specific custom source (e.g. ssh, bitwarden)
        #[arg(short, long)]
        source: Option<String>,

        /// Root directory for file searches
        #[arg(short, long)]
        root: Option<PathBuf>,

        /// Glob pattern for file searches (e.g. '*.backup')
        #[arg(short = 'g', long)]
        glob: Option<String>,

        /// Scan the filesystem directly instead of using the cached file index
        #[arg(long)]
        no_cache: bool,

        /// Do not use the default ignore list (.git, node_modules, etc.)
        #[arg(long)]
        no_default_ignores: bool,

        /// Include dotfiles and dot-directories in file searches
        #[arg(long)]
        hidden: bool,

        /// Maximum number of results to return
        #[arg(short, long, default_value_t = 50)]
        limit: usize,
    },
    /// Execute action for selected item
    Execute {
        /// The source name (omit for default file open)
        #[arg(short, long)]
        source: Option<String>,

        /// The selected item value
        #[arg(short, long)]
        item: String,
    },
    /// Force rebuild the cache
    RebuildCache {
        /// Root directory to scan (default is user's home directory)
        #[arg(short, long)]
        root: Option<PathBuf>,

        /// Do not use the default ignore list (.git, node_modules, etc.)
        #[arg(long)]
        no_default_ignores: bool,

        /// Index dotfiles and dot-directories as well
        #[arg(long)]
        hidden: bool,
    },
    /// List available custom source configurations
    ListScripts,
}

impl Default for Commands {
    fn default() -> Self {
        Self::Search {
            query: String::new(),
            source: None,
            root: None,
            glob: None,
            no_cache: false,
            no_default_ignores: false,
            hidden: false,
            limit: 50,
        }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct Config {
    #[serde(default)]
    sources: Vec<SourceConfig>,
    #[serde(default)]
    ignore_dirs: Vec<String>,
    #[serde(default = "default_true")]
    use_default_ignores: bool,
    #[serde(default)]
    root_dir: Option<PathBuf>,
    #[serde(default)]
    cache_ttl_seconds: Option<u64>,
    /// Whether file searches include dotfiles/dot-directories by default.
    /// `--hidden` on the command line turns it on for a single invocation.
    #[serde(default)]
    include_hidden: bool,

    /// Escape-hatch list of desktop launchers to hide, matched against the
    /// desktop-file ID (e.g. `foo.desktop`) or file basename, case-insensitively.
    ///
    /// Visibility is normally decided by the XDG Desktop Entry specification
    /// (`Type`, `Hidden`, `NoDisplay`, `TryExec`, `OnlyShowIn`/`NotShowIn`);
    /// this list only exists for the rare launcher that is mislabeled upstream.
    #[serde(default)]
    desktop_exclude: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            sources: Vec::new(),
            ignore_dirs: Vec::new(),
            use_default_ignores: true,
            root_dir: None,
            cache_ttl_seconds: None,
            include_hidden: false,
            desktop_exclude: Vec::new(),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct SourceConfig {
    name: String,
    icon: String,
    description: Option<String>,
    script: String,
    action: String,
}

fn load_config() -> Config {
    let config_dir = dirs::config_dir()
        .expect("Could not determine config directory")
        .join("apicker");

    let config_path = config_dir.join("config.toml");
    if !config_path.exists() {
        return Config::default();
    }

    match fs::read_to_string(&config_path) {
        Ok(content) => match toml::from_str(&content) {
            Ok(config) => config,
            Err(e) => {
                eprintln!("Error parsing config.toml: {}", e);
                Config::default()
            }
        },
        Err(e) => {
            eprintln!("Error reading config.toml: {}", e);
            Config::default()
        }
    }
}

fn run_source_script(script: &str) -> std::io::Result<Vec<String>> {
    let output = std::process::Command::new("sh")
        .arg("-c")
        .arg(script)
        .output()?;

    if !output.status.success() {
        return Err(std::io::Error::other(format!(
            "Script failed with exit code: {:?}",
            output.status.code()
        )));
    }

    let stdout_str = String::from_utf8_lossy(&output.stdout);
    let items = stdout_str
        .lines()
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .collect();

    Ok(items)
}

/// Shell-escape a string by wrapping it in single quotes with proper escaping.
fn shell_escape(s: &str) -> String {
    // Replace each ' with '\'' (end quote, escaped quote, start quote)
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Launch a desktop entry by desktop-file path.
///
/// The `.desktop` file is re-parsed and its `Exec` key expanded into an argv
/// vector according to the Desktop Entry specification. The argv is spawned
/// directly — never through `sh -c` — so `Exec` values cannot inject shell
/// commands and field codes are handled per spec rather than by string hacks.
fn launch_desktop_entry(desktop_path: &str) {
    // ponytail: re-reads the entry at launch time; cache if launch latency ever matters.
    let path = PathBuf::from(desktop_path);
    let content = match fs::read_to_string(&path) {
        Ok(content) => content,
        Err(e) => {
            eprintln!("Failed to read desktop entry '{}': {}", desktop_path, e);
            return;
        }
    };

    let entry = desktop_spec::parse_desktop_entry(&content);
    let Some(argv) = desktop_spec::exec_to_argv(
        entry.exec.as_deref().unwrap_or_default(),
        &entry,
        Some(path.as_path()),
    ) else {
        eprintln!(
            "Desktop entry '{}' has no runnable Exec command.",
            desktop_path
        );
        return;
    };

    let (program, args) = argv.split_first().expect("argv is non-empty");
    let mut command = std::process::Command::new(program);
    command.args(args);
    match spawn_detached(&mut command) {
        Ok(_) => {}
        Err(e) => eprintln!("Failed to launch '{}': {}", program, e),
    }
}

/// Start a launcher without inheriting apicker's stdout/stderr.
///
/// apicker exits immediately, but a chatty child would otherwise keep the
/// caller's pipe open and block once its buffer fills. stdin is inherited so
/// actions that prompt (e.g. `sudo`) still work from a terminal.
fn spawn_detached(command: &mut std::process::Command) -> std::io::Result<std::process::Child> {
    command.stdout(Stdio::null()).stderr(Stdio::null()).spawn()
}

const DEFAULT_IGNORES: &[&str] = &[
    "node_modules",
    "target",
    ".git",
    "build",
    "dist",
    ".cache",
    "proc",
    "sys",
    "dev",
    "run",
    "tmp",
];

/// Merge configured ignore dirs with the built-in list, keeping order and
/// dropping duplicates. Used by both `search` and `rebuild-cache` so the two
/// always agree on what the cache contains.
fn resolve_ignores(config: &Config, no_default_ignores: bool) -> Vec<String> {
    let mut ignores = config.ignore_dirs.clone();
    if config.use_default_ignores && !no_default_ignores {
        for &dir in DEFAULT_IGNORES {
            if !ignores.iter().any(|d| d == dir) {
                ignores.push(dir.to_string());
            }
        }
    }
    ignores
}

fn main() {
    let cli = Cli::parse();
    let config = load_config();

    let command = cli.command.unwrap_or_default();

    match command {
        Commands::Search {
            query,
            source,
            root,
            glob,
            no_cache,
            no_default_ignores,
            hidden,
            limit,
        } => {
            let matcher = SkimMatcherV2::default().ignore_case();
            let mut results: Vec<PickerItem> = Vec::new();
            let mru = MruCache::load();
            let file_options_requested = root.is_some() || glob.is_some() || no_cache;
            let include_hidden = hidden || config.include_hidden;

            let final_ignores = resolve_ignores(&config, no_default_ignores);

            if let Some(source_name) = source {
                if source_name == "desktop" {
                    let desktop_provider = DesktopProvider {
                        exclude: &config.desktop_exclude,
                        desktops: Vec::new(),
                    };
                    results.extend(desktop_provider.fetch(&query, &mru, &matcher));
                } else if source_name == "files" {
                    let files_provider = FilesProvider {
                        ignore_dirs: &final_ignores,
                        root_dir: root.as_deref().or(config.root_dir.as_deref()),
                        cache_ttl_seconds: config.cache_ttl_seconds,
                        glob: glob.as_deref(),
                        use_cache: !no_cache,
                        include_hidden,
                    };
                    results.extend(files_provider.fetch(&query, &mru, &matcher));
                } else if let Some(src_config) =
                    config.sources.iter().find(|s| s.name == source_name)
                {
                    match run_source_script(&src_config.script) {
                        Ok(items) => {
                            for item in items {
                                let count = mru.get(&item);
                                let boost = (count as i64) * MRU_BOOST_FACTOR;
                                let score = if let Some(s) = matcher.fuzzy_match(&item, &query) {
                                    s + boost
                                } else if query.is_empty() {
                                    boost
                                } else {
                                    continue;
                                };

                                results.push(PickerItem {
                                    text: item.clone(),
                                    path: item,
                                    icon: src_config.icon.clone(),
                                    is_dir: false,
                                    is_submenu: false,
                                    target_source: None,
                                    score,
                                });
                            }
                        }
                        Err(e) => {
                            eprintln!("Error running source script for '{}': {}", source_name, e);
                        }
                    }
                } else {
                    eprintln!("Source '{}' not found in configuration.", source_name);
                }
            } else {
                let search_query = query.strip_prefix(':').unwrap_or(&query);
                let show_sources = query.is_empty() || search_query.len() < query.len();

                if show_sources && !file_options_requested {
                    for src in &config.sources {
                        let matches = search_query.is_empty()
                            || matcher.fuzzy_match(&src.name, search_query).is_some();
                        if matches {
                            let display_desc =
                                src.description.as_deref().unwrap_or("Custom Script Source");
                            let count = mru.get(&src.name);
                            let boost = (count as i64) * MRU_BOOST_FACTOR;
                            results.push(PickerItem {
                                text: format!("Source: {} ({})", src.name, display_desc),
                                path: src.name.clone(),
                                icon: src.icon.clone(),
                                is_dir: false,
                                is_submenu: true,
                                target_source: Some(src.name.clone()),
                                score: SOURCE_BASE_SCORE
                                    + (SOURCE_NAME_LENGTH_BASE - src.name.len() as i64)
                                    + boost,
                            });
                        }
                    }
                }

                if !file_options_requested {
                    let desktop_provider = DesktopProvider {
                        exclude: &config.desktop_exclude,
                        desktops: Vec::new(),
                    };
                    results.extend(desktop_provider.fetch(search_query, &mru, &matcher));
                }

                let files_provider = FilesProvider {
                    ignore_dirs: &final_ignores,
                    root_dir: root.as_deref().or(config.root_dir.as_deref()),
                    cache_ttl_seconds: config.cache_ttl_seconds,
                    glob: glob.as_deref(),
                    use_cache: !no_cache,
                    include_hidden,
                };
                results.extend(files_provider.fetch(search_query, &mru, &matcher));
            }

            let by_rank = |a: &PickerItem, b: &PickerItem| {
                b.score
                    .cmp(&a.score)
                    .then_with(|| a.path.len().cmp(&b.path.len()))
            };

            // Partition down to the top `limit` before sorting: a home-dir
            // scan can yield hundreds of thousands of items per keystroke.
            if results.len() > limit {
                results.select_nth_unstable_by(limit, by_rank);
                results.truncate(limit);
            }
            results.sort_by(by_rank);

            match serde_json::to_string(&results) {
                Ok(json) => println!("{}", json),
                Err(e) => eprintln!("Error serializing search results: {}", e),
            }
        }
        Commands::Execute { source, item } => {
            // Desktop entries are scored against their resolved argv (see
            // `desktop::build_item`), so the MRU write must use the same key
            // instead of the desktop-file path.
            let mru_key = match source.as_deref() {
                Some("desktop") => desktop::desktop_mru_key(Path::new(&item)),
                _ => None,
            }
            .unwrap_or_else(|| item.clone());

            let mut mru = MruCache::load();
            mru.increment(&mru_key);
            mru.save();

            if source.as_deref() == Some("desktop") {
                launch_desktop_entry(&item);
            } else if let Some(src_name) = source {
                if let Some(src_config) = config.sources.iter().find(|s| s.name == src_name) {
                    let escaped_item = shell_escape(&item);
                    let expanded_cmd = src_config.action.replace("{item}", &escaped_item);
                    let mut command = std::process::Command::new("sh");
                    command.arg("-c").arg(&expanded_cmd);
                    match spawn_detached(&mut command) {
                        Ok(_) => {}
                        Err(e) => eprintln!("Failed to spawn action process: {}", e),
                    }
                } else {
                    eprintln!("Source '{}' not found in config.", src_name);
                }
            } else {
                let mut command = std::process::Command::new("xdg-open");
                command.arg(&item);
                match spawn_detached(&mut command) {
                    Ok(_) => {}
                    Err(e) => eprintln!("Failed to run xdg-open for '{}': {}", item, e),
                }
            }
        }
        Commands::RebuildCache {
            root,
            no_default_ignores,
            hidden,
        } => {
            let root_dir = root
                .or_else(|| config.root_dir.clone())
                .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from("/")));
            let final_ignores = resolve_ignores(&config, no_default_ignores);
            let include_hidden = hidden || config.include_hidden;
            println!("Scanning directory {:?} to rebuild cache...", root_dir);
            match rebuild_cache(&root_dir, &final_ignores, include_hidden) {
                Ok(paths) => println!("Cache rebuilt successfully with {} items.", paths.len()),
                Err(e) => eprintln!("Failed to rebuild cache: {}", e),
            }
        }
        Commands::ListScripts => match serde_json::to_string(&config.sources) {
            Ok(json) => println!("{}", json),
            Err(e) => eprintln!("Error serializing scripts: {}", e),
        },
    }
}
