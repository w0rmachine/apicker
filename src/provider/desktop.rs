use crate::core::PickerItem;
use crate::mru::MruCache;
use crate::provider::Provider;
use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use std::fs;
use std::path::{Path, PathBuf};

pub struct DesktopProvider;

impl Provider for DesktopProvider {
    fn fetch(&self, query: &str, mru: &MruCache) -> Vec<PickerItem> {
        let mut results = Vec::new();
        let matcher = SkimMatcherV2::default();

        let dirs = vec![
            PathBuf::from("/usr/share/applications/"),
            dirs::data_local_dir()
                .unwrap_or_else(|| PathBuf::from("/home/mwu/.local/share"))
                .join("applications/"),
        ];

        for dir in dirs {
            if let Ok(entries) = fs::read_dir(dir) {
                for entry in entries.filter_map(Result::ok) {
                    let path = entry.path();
                    if path.extension().and_then(|e| e.to_str()) == Some("desktop") {
                        if let Some(item) = parse_desktop_file(&path, query, &matcher, mru) {
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
            in_desktop_entry = false;
        }

        if in_desktop_entry {
            if line.starts_with("Name=") && name.is_empty() {
                name = line["Name=".len()..].to_string();
            } else if line.starts_with("Icon=") && icon.is_empty() {
                icon = line["Icon=".len()..].to_string();
            } else if line.starts_with("Exec=") && exec.is_empty() {
                exec = line["Exec=".len()..].to_string();
            }
        }
    }

    if name.is_empty() || exec.is_empty() {
        return None;
    }

    // Strip %u, %F, etc.
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
    let boost = (count * 50) as i64;

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
