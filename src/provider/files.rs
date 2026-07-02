use crate::core::PickerItem;
use crate::mru::MruCache;
use crate::provider::Provider;
use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

use crate::MRU_BOOST_FACTOR;

pub struct FilesProvider<'a> {
    pub ignore_dirs: &'a [String],
}

struct CacheEntry {
    path: String,
    is_dir: bool,
}

impl<'a> Provider for FilesProvider<'a> {
    fn fetch(&self, query: &str, mru: &MruCache, matcher: &SkimMatcherV2) -> Vec<PickerItem> {
        let mut results = Vec::new();

        match load_cache(self.ignore_dirs) {
            Ok(entries) => {
                for entry in entries {
                    let path = Path::new(&entry.path);
                    let file_name = path
                        .file_name()
                        .map(|f| f.to_string_lossy().into_owned())
                        .unwrap_or_else(|| entry.path.clone());

                    let count = mru.get(&entry.path);
                    let boost = (count as i64) * MRU_BOOST_FACTOR;

                    let (score, match_target) = if let Some(s) = matcher.fuzzy_match(&file_name, query) {
                        (s + boost, file_name)
                    } else if let Some(s) = matcher.fuzzy_match(&entry.path, query) {
                        (s - 10 + boost, entry.path.clone())
                    } else if query.is_empty() {
                        (boost, file_name)
                    } else {
                        continue;
                    };

                    let icon = determine_icon(path, entry.is_dir);
                    results.push(PickerItem {
                        text: match_target,
                        path: entry.path,
                        icon,
                        is_dir: entry.is_dir,
                        is_submenu: false,
                        target_source: None,
                        score,
                    });
                }
            }
            Err(e) => {
                eprintln!("Failed to load cache: {}", e);
            }
        }

        results
    }
}

fn get_cache_dir() -> PathBuf {
    dirs::cache_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join(".cache")))
        .expect("Could not determine cache directory")
        .join("apicker")
}

pub fn get_cache_file() -> PathBuf {
    get_cache_dir().join("paths.txt")
}

pub fn rebuild_cache(root_dir: &Path, ignore_dirs: &[String]) -> io::Result<Vec<String>> {
    let cache_dir = get_cache_dir();
    fs::create_dir_all(&cache_dir)?;
    let cache_file_path = get_cache_file();
    let mut file = File::create(&cache_file_path)?;

    let mut paths = Vec::new();
    for entry in WalkDir::new(root_dir)
        .into_iter()
        .filter_entry(|e| {
            let file_name = e.file_name().to_string_lossy();
            if file_name.starts_with('.') && file_name != "." {
                return false;
            }
            if ignore_dirs.iter().any(|ignore| ignore == file_name.as_ref()) {
                return false;
            }
            true
        })
        .filter_map(|e| e.ok())
    {
        let path_str = entry.path().to_string_lossy().into_owned();
        let type_flag = if entry.file_type().is_dir() { 'D' } else { 'F' };
        writeln!(file, "{}\t{}", path_str, type_flag)?;
        paths.push(path_str);
    }
    Ok(paths)
}

fn load_cache(ignore_dirs: &[String]) -> io::Result<Vec<CacheEntry>> {
    let cache_file_path = get_cache_file();
    if !cache_file_path.exists() {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
        rebuild_cache(&home, ignore_dirs)?;
        // Re-read the freshly built cache
        return load_cache_from_file(&cache_file_path);
    }

    load_cache_from_file(&cache_file_path)
}

fn load_cache_from_file(path: &Path) -> io::Result<Vec<CacheEntry>> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut entries = Vec::new();
    for line in reader.lines().map_while(Result::ok) {
        if let Some((path_str, flag)) = line.rsplit_once('\t') {
            entries.push(CacheEntry {
                path: path_str.to_string(),
                is_dir: flag == "D",
            });
        } else {
            // Backwards compatibility: old cache format without type flag
            entries.push(CacheEntry {
                path: line,
                is_dir: false,
            });
        }
    }
    Ok(entries)
}

fn determine_icon(path: &Path, is_dir: bool) -> String {
    if is_dir {
        "folder".to_string()
    } else if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        match ext.to_lowercase().as_str() {
            "rs" | "go" | "py" | "js" | "ts" | "cpp" | "c" | "h" | "sh" | "nix" => {
                "code-context".to_string()
            }
            "png" | "jpg" | "jpeg" | "gif" | "svg" => "image-x-generic".to_string(),
            "mp4" | "mkv" | "avi" => "video-x-generic".to_string(),
            "mp3" | "flac" | "wav" => "audio-x-generic".to_string(),
            "pdf" | "epub" => "document-properties".to_string(),
            "zip" | "tar" | "gz" | "xz" => "package-x-generic".to_string(),
            _ => "text-x-generic".to_string(),
        }
    } else {
        "text-x-generic".to_string()
    }
}
