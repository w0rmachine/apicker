use crate::core::PickerItem;
use crate::mru::MruCache;
use crate::provider::Provider;
use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};
use walkdir::WalkDir;

use crate::MRU_BOOST_FACTOR;

const CACHE_FORMAT_VERSION: u32 = 2;
const DEFAULT_CACHE_TTL_SECONDS: u64 = 60 * 60;

pub struct FilesProvider<'a> {
    pub ignore_dirs: &'a [String],
    pub root_dir: Option<&'a Path>,
    pub cache_ttl_seconds: Option<u64>,
    pub glob: Option<&'a str>,
    pub use_cache: bool,
}

#[derive(Clone)]
struct CacheEntry {
    path: String,
    is_dir: bool,
}

#[derive(Serialize, Deserialize)]
struct CacheMetadata {
    version: u32,
    root: String,
    ignore_dirs: Vec<String>,
    root_modified: Option<u64>,
    built_at: u64,
}

impl<'a> Provider for FilesProvider<'a> {
    fn fetch(&self, query: &str, mru: &MruCache, matcher: &SkimMatcherV2) -> Vec<PickerItem> {
        let mut results = Vec::new();

        match load_entries(
            self.root_dir,
            self.ignore_dirs,
            self.cache_ttl_seconds,
            self.use_cache,
        ) {
            Ok(entries) => {
                for entry in entries {
                    let path = Path::new(&entry.path);
                    if !matches_glob_filter(path, self.glob) {
                        continue;
                    }

                    let file_name = path
                        .file_name()
                        .map(|f| f.to_string_lossy().into_owned())
                        .unwrap_or_else(|| entry.path.clone());

                    let count = mru.get(&entry.path);
                    let boost = (count as i64) * MRU_BOOST_FACTOR;

                    let (score, match_target) =
                        if let Some(s) = matcher.fuzzy_match(&file_name, query) {
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

fn get_cache_meta_file() -> PathBuf {
    get_cache_dir().join("paths.json")
}

pub fn rebuild_cache(root_dir: &Path, ignore_dirs: &[String]) -> io::Result<Vec<String>> {
    let cache_dir = get_cache_dir();
    fs::create_dir_all(&cache_dir)?;
    let cache_file_path = get_cache_file();
    let cache_file_tmp = temporary_path_for(&cache_file_path);
    let mut file = File::create(&cache_file_tmp)?;

    let entries = scan_entries(root_dir, ignore_dirs);
    for entry in &entries {
        let type_flag = if entry.is_dir { 'D' } else { 'F' };
        writeln!(file, "{}\t{}", entry.path, type_flag)?;
    }
    file.flush()?;
    fs::rename(cache_file_tmp, cache_file_path)?;

    write_cache_metadata(root_dir, ignore_dirs)?;
    Ok(entries.into_iter().map(|entry| entry.path).collect())
}

fn load_entries(
    root_dir: Option<&Path>,
    ignore_dirs: &[String],
    cache_ttl_seconds: Option<u64>,
    use_cache: bool,
) -> io::Result<Vec<CacheEntry>> {
    let root_dir = root_dir
        .map(Path::to_path_buf)
        .unwrap_or_else(default_cache_root);

    if !use_cache {
        return Ok(scan_entries(&root_dir, ignore_dirs));
    }

    load_cache(&root_dir, ignore_dirs, cache_ttl_seconds)
}

fn load_cache(
    root_dir: &Path,
    ignore_dirs: &[String],
    cache_ttl_seconds: Option<u64>,
) -> io::Result<Vec<CacheEntry>> {
    let cache_file_path = get_cache_file();

    if should_rebuild_cache(&root_dir, ignore_dirs, cache_ttl_seconds) {
        rebuild_cache(root_dir, ignore_dirs)?;
    }

    match load_cache_from_file(&cache_file_path) {
        Ok(entries) => Ok(entries),
        Err(err) => {
            eprintln!("Cache read failed, rebuilding: {}", err);
            rebuild_cache(root_dir, ignore_dirs)?;
            load_cache_from_file(&cache_file_path)
        }
    }
}

fn scan_entries(root_dir: &Path, ignore_dirs: &[String]) -> Vec<CacheEntry> {
    WalkDir::new(root_dir)
        .into_iter()
        .filter_entry(|e| {
            let file_name = e.file_name().to_string_lossy();
            if file_name.starts_with('.') && file_name != "." {
                return false;
            }
            if ignore_dirs
                .iter()
                .any(|ignore| ignore == file_name.as_ref())
            {
                return false;
            }
            true
        })
        .filter_map(|entry| entry.ok())
        .map(|entry| CacheEntry {
            path: entry.path().to_string_lossy().into_owned(),
            is_dir: entry.file_type().is_dir(),
        })
        .collect()
}

fn default_cache_root() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
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

fn should_rebuild_cache(
    root_dir: &Path,
    ignore_dirs: &[String],
    cache_ttl_seconds: Option<u64>,
) -> bool {
    if !get_cache_file().exists() {
        return true;
    }

    let metadata = match read_cache_metadata() {
        Ok(metadata) => metadata,
        Err(_) => return true,
    };

    if metadata.version != CACHE_FORMAT_VERSION {
        return true;
    }

    if metadata.root != root_dir.to_string_lossy() {
        return true;
    }

    if metadata.ignore_dirs != normalized_ignore_dirs(ignore_dirs) {
        return true;
    }

    if metadata.root_modified != modified_seconds(root_dir) {
        return true;
    }

    let ttl = cache_ttl_seconds.unwrap_or(DEFAULT_CACHE_TTL_SECONDS);
    ttl > 0 && now_seconds().saturating_sub(metadata.built_at) >= ttl
}

fn read_cache_metadata() -> io::Result<CacheMetadata> {
    let content = fs::read_to_string(get_cache_meta_file())?;
    serde_json::from_str(&content).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

fn write_cache_metadata(root_dir: &Path, ignore_dirs: &[String]) -> io::Result<()> {
    let metadata = CacheMetadata {
        version: CACHE_FORMAT_VERSION,
        root: root_dir.to_string_lossy().into_owned(),
        ignore_dirs: normalized_ignore_dirs(ignore_dirs),
        root_modified: modified_seconds(root_dir),
        built_at: now_seconds(),
    };

    let metadata_path = get_cache_meta_file();
    let metadata_tmp = temporary_path_for(&metadata_path);
    let content = serde_json::to_string(&metadata)?;
    fs::write(&metadata_tmp, content)?;
    fs::rename(metadata_tmp, metadata_path)
}

fn temporary_path_for(path: &Path) -> PathBuf {
    let mut temporary_path = path.to_path_buf();
    let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("tmp");
    temporary_path.set_extension(format!(
        "{}.{}.{}.tmp",
        extension,
        process::id(),
        now_nanos()
    ));
    temporary_path
}

fn normalized_ignore_dirs(ignore_dirs: &[String]) -> Vec<String> {
    let mut dirs = ignore_dirs.to_vec();
    dirs.sort_unstable();
    dirs.dedup();
    dirs
}

fn modified_seconds(path: &Path) -> Option<u64> {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(system_time_seconds)
}

fn now_seconds() -> u64 {
    system_time_seconds(SystemTime::now()).unwrap_or(0)
}

fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

fn system_time_seconds(time: SystemTime) -> Option<u64> {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
}

fn matches_glob_filter(path: &Path, pattern: Option<&str>) -> bool {
    let Some(pattern) = pattern else {
        return true;
    };

    if pattern.contains('/') {
        let path = path.to_string_lossy();
        wildcard_match(pattern, &path)
    } else {
        path.file_name()
            .map(|name| wildcard_match(pattern, &name.to_string_lossy()))
            .unwrap_or(false)
    }
}

fn wildcard_match(pattern: &str, text: &str) -> bool {
    let pattern = pattern.as_bytes();
    let text = text.as_bytes();
    let (mut pattern_index, mut text_index) = (0, 0);
    let mut star_index = None;
    let mut star_text_index = 0;

    while text_index < text.len() {
        if pattern_index < pattern.len()
            && (pattern[pattern_index] == b'?' || pattern[pattern_index] == text[text_index])
        {
            pattern_index += 1;
            text_index += 1;
        } else if pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
            star_index = Some(pattern_index);
            pattern_index += 1;
            star_text_index = text_index;
        } else if let Some(star) = star_index {
            pattern_index = star + 1;
            star_text_index += 1;
            text_index = star_text_index;
        } else {
            return false;
        }
    }

    while pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
        pattern_index += 1;
    }

    pattern_index == pattern.len()
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

#[cfg(test)]
mod tests {
    use super::{matches_glob_filter, wildcard_match};
    use std::path::Path;

    #[test]
    fn wildcard_supports_star_and_question_mark() {
        assert!(wildcard_match("*.backup", "database.backup"));
        assert!(wildcard_match("file-?.backup", "file-1.backup"));
        assert!(!wildcard_match("file-?.backup", "file-12.backup"));
        assert!(!wildcard_match("*.backup", "database.backup.old"));
    }

    #[test]
    fn glob_without_slash_matches_file_name() {
        assert!(matches_glob_filter(
            Path::new("/tmp/project/database.backup"),
            Some("*.backup")
        ));
        assert!(!matches_glob_filter(
            Path::new("/tmp/project/database.txt"),
            Some("*.backup")
        ));
    }

    #[test]
    fn glob_with_slash_matches_full_path() {
        assert!(matches_glob_filter(
            Path::new("/tmp/project/database.backup"),
            Some("/tmp/*/*.backup")
        ));
        assert!(!matches_glob_filter(
            Path::new("/var/project/database.backup"),
            Some("/tmp/*/*.backup")
        ));
    }
}
