use crate::core::PickerItem;
use crate::mru::MruCache;
use crate::provider::Provider;
use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use std::fs;
use std::path::{Path, PathBuf};

use crate::MRU_BOOST_FACTOR;

pub struct DesktopProvider;

impl Provider for DesktopProvider {
    fn fetch(&self, query: &str, mru: &MruCache, matcher: &SkimMatcherV2) -> Vec<PickerItem> {
        let mut results = Vec::new();

        let mut app_dirs = Vec::new();

        // User-specific applications
        if let Some(local_dir) = dirs::data_local_dir() {
            app_dirs.push(local_dir.join("applications"));
        } else if let Some(home) = dirs::home_dir() {
            app_dirs.push(home.join(".local/share/applications"));
        }

        // System-wide and profile applications
        if let Ok(xdg_data_dirs) = std::env::var("XDG_DATA_DIRS") {
            for dir in xdg_data_dirs.split(':') {
                if !dir.is_empty() {
                    app_dirs.push(PathBuf::from(dir).join("applications"));
                }
            }
        } else {
            app_dirs.push(PathBuf::from("/usr/local/share/applications"));
            app_dirs.push(PathBuf::from("/usr/share/applications"));
        }

        for dir in app_dirs {
            if let Ok(entries) = fs::read_dir(dir) {
                for entry in entries.filter_map(Result::ok) {
                    let path = entry.path();
                    if path.extension().and_then(|e| e.to_str()) == Some("desktop") {
                        if let Some(item) = parse_desktop_file(&path, query, matcher, mru) {
                            results.push(item);
                        }
                    }
                }
            }
        }

        results
    }
}

fn parse_desktop_file(
    path: &Path,
    query: &str,
    matcher: &SkimMatcherV2,
    mru: &MruCache,
) -> Option<PickerItem> {
    let content = fs::read_to_string(path).ok()?;
    let mut name = String::new();
    let mut icon = String::new();
    let mut exec = String::new();

    let mut in_desktop_entry = false;
    for line in content.lines() {
        let line = line.trim();
        if line == "[Desktop Entry]" {
            in_desktop_entry = true;
            continue;
        }
        if line.starts_with('[') {
            if in_desktop_entry {
                // We've left [Desktop Entry], no need to parse further sections
                break;
            }
            continue;
        }

        if in_desktop_entry {
            if let Some(val) = line.strip_prefix("Name=") {
                if name.is_empty() {
                    name = val.to_string();
                }
            } else if let Some(val) = line.strip_prefix("Icon=") {
                if icon.is_empty() {
                    icon = val.to_string();
                }
            } else if let Some(val) = line.strip_prefix("Exec=") {
                if exec.is_empty() {
                    exec = val.to_string();
                }
            }
        }
    }

    if name.is_empty() || exec.is_empty() {
        return None;
    }

    // Strip %u, %F, etc. field codes
    let exec = exec
        .replace("%u", "")
        .replace("%U", "")
        .replace("%f", "")
        .replace("%F", "")
        .replace("%c", "")
        .replace("%k", "")
        .replace("%i", "")
        .trim()
        .to_string();

    let count = mru.get(&exec);
    let boost = (count as i64) * MRU_BOOST_FACTOR;

    let score = if query.is_empty() {
        boost
    } else if let Some(s) = matcher.fuzzy_match(&name, query) {
        s + boost
    } else if let Some(s) = matcher.fuzzy_match(&exec, query) {
        (s - 10) + boost
    } else {
        return None;
    };

    if icon.is_empty() {
        icon = "application-x-executable".to_string();
    }

    Some(PickerItem {
        text: name,
        path: exec,
        icon,
        is_dir: false,
        is_submenu: false,
        target_source: None,
        score,
    })
}
