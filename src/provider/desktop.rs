use crate::core::PickerItem;
use crate::mru::MruCache;
use crate::provider::desktop_spec::{self, DesktopEntry};
use crate::provider::Provider;
use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::MRU_BOOST_FACTOR;

/// Default icon used when a desktop entry does not declare one.
const DEFAULT_ICON: &str = "application-x-executable";

/// Indexes `.desktop` launchers following the XDG Desktop Entry specification.
///
/// * Directory precedence is handled by [`application_dirs`] so that user
///   overrides win over profile/system entries.
/// * De-duplication is by desktop-file ID (`xdg-desktop-portal-gtk.desktop`,
///   `org.gnome.Foo.desktop`, ...), not by raw filename ordering.
/// * Visibility is delegated to
///   [`desktop_spec::is_visible`] so `Type`, `Hidden`, `NoDisplay`, `TryExec`
///   and `OnlyShowIn`/`NotShowIn` are all honored uniformly.
#[derive(Default)]
pub struct DesktopProvider<'a> {
    /// Explicit exclusions supplied by configuration. Matched against either
    /// the desktop-file ID (`foo.desktop`) or the basename, case-insensitively.
    ///
    /// This is an escape hatch layered on top of the specification rules; it is
    /// never the primary filtering mechanism.
    pub exclude: &'a [String],
    /// Desktop names that are considered active. Defaults to
    /// `$XDG_CURRENT_DESKTOP` when empty.
    pub desktops: Vec<String>,
}

impl Provider for DesktopProvider<'_> {
    fn fetch(&self, query: &str, mru: &MruCache, matcher: &SkimMatcherV2) -> Vec<PickerItem> {
        self.collect(&application_dirs(), query, mru, matcher)
    }
}

impl DesktopProvider<'_> {
    /// Index the given application directories, highest precedence first.
    ///
    /// Split out from [`Provider::fetch`] so tests can supply fixture
    /// directories instead of the real XDG locations.
    fn collect(
        &self,
        dirs: &[PathBuf],
        query: &str,
        mru: &MruCache,
        matcher: &SkimMatcherV2,
    ) -> Vec<PickerItem> {
        let desktops = self.active_desktops();
        let desktop_refs: Vec<&str> = desktops.iter().map(String::as_str).collect();
        let excluded: HashSet<String> = self
            .exclude
            .iter()
            .map(|s| s.trim().to_ascii_lowercase())
            .filter(|s| !s.is_empty())
            .collect();

        let mut items = Vec::new();
        // A desktop-file ID seen at a higher-precedence directory may only be
        // overridden, never re-added, by lower-precedence directories.
        let mut resolved: HashSet<String> = HashSet::new();

        for dir in dirs {
            let Ok(read_dir) = fs::read_dir(dir) else {
                continue;
            };

            // `read_dir` order is unspecified; sort for deterministic output.
            let mut paths: Vec<PathBuf> = read_dir
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("desktop"))
                .collect();
            paths.sort();

            for path in paths {
                let Some(id) = desktop_file_id(&path) else {
                    continue;
                };

                // First writer wins: a higher-precedence entry with this ID
                // (even a hidden one) suppresses every lower-precedence copy.
                if !resolved.insert(id.clone()) {
                    continue;
                }

                // Config escape hatch: match the desktop-file ID, the basename,
                // or the basename with its `.desktop` suffix stripped.
                if is_excluded(&excluded, &id, &path) {
                    continue;
                }

                let Some(entry) = load_entry(&path) else {
                    continue;
                };

                if !desktop_spec::is_visible(&entry, &desktop_refs) {
                    continue;
                }

                if let Some(item) = build_item(&entry, &path, query, matcher, mru) {
                    items.push(item);
                }
            }
        }

        items
    }

    /// The active desktop names, falling back to `$XDG_CURRENT_DESKTOP`.
    fn active_desktops(&self) -> Vec<String> {
        if !self.desktops.is_empty() {
            return self.desktops.clone();
        }

        std::env::var("XDG_CURRENT_DESKTOP")
            .ok()
            .map(|value| {
                value
                    .split(':')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(ToString::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// Application directories in XDG precedence order: highest priority first.
///
/// The local data directory (`~/.local/share/applications`) always wins over
/// entries listed in `$XDG_DATA_DIRS`. Duplicate directories are collapsed
/// while preserving the first (highest-precedence) occurrence.
pub fn application_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();

    let data_home =
        dirs::data_local_dir().or_else(|| dirs::home_dir().map(|home| home.join(".local/share")));
    if let Some(data_home) = data_home {
        push_unique(&mut dirs, data_home.join("applications"));
    }

    let data_dirs = std::env::var("XDG_DATA_DIRS")
        .unwrap_or_else(|_| "/usr/local/share:/usr/share".to_string());
    for dir in data_dirs.split(':') {
        if dir.trim().is_empty() {
            continue;
        }
        push_unique(&mut dirs, PathBuf::from(dir).join("applications"));
    }

    dirs
}

fn push_unique(dirs: &mut Vec<PathBuf>, candidate: PathBuf) {
    if !dirs.iter().any(|existing| existing == &candidate) {
        dirs.push(candidate);
    }
}

/// Compute the desktop-file ID for a path.
///
/// The ID is the path relative to the `applications` directory, with `/`
/// replaced by `-`. Subdirectories are included so that
/// `applications/kde/foo.desktop` yields `kde-foo.desktop`, matching the
/// specification. If no `applications` ancestor is found, the basename is used
/// as a best-effort fallback.
fn desktop_file_id(path: &Path) -> Option<String> {
    let file_name = path.file_name()?.to_str()?;

    // Walk up from the file collecting directory components until we hit the
    // `applications` directory that roots this tree.
    let mut components: Vec<&str> = Vec::new();
    let mut current = path.parent();
    while let Some(dir) = current {
        let Some(name) = dir.file_name().and_then(|n| n.to_str()) else {
            break;
        };
        if name.eq_ignore_ascii_case("applications") {
            components.reverse();
            components.push(file_name);
            return Some(components.join("-"));
        }
        components.push(name);
        current = dir.parent();
    }

    // Non-standard layout: fall back to the basename.
    Some(file_name.to_string())
}

/// Whether an entry is covered by the user-configured exclusion list.
///
/// `excluded` holds lowercased, trimmed patterns. A pattern matches when it
/// equals the desktop-file ID (`foo.desktop`, `kde-kate.desktop`), the file's
/// basename (`foo.desktop`), or the basename without its `.desktop` extension
/// (`foo`). This makes the escape hatch forgiving about how the user names the
/// launcher.
fn is_excluded(excluded: &HashSet<String>, id: &str, path: &Path) -> bool {
    if excluded.is_empty() {
        return false;
    }

    let id = id.to_ascii_lowercase();
    if excluded.contains(&id) {
        return true;
    }

    let Some(basename) = path.file_name().and_then(|f| f.to_str()) else {
        return false;
    };
    let basename = basename.to_ascii_lowercase();
    if excluded.contains(&basename) {
        return true;
    }

    let stem = basename
        .strip_suffix(".desktop")
        .unwrap_or(basename.as_str());
    excluded.contains(stem)
}

fn load_entry(path: &Path) -> Option<DesktopEntry> {
    let content = fs::read_to_string(path).ok()?;
    Some(desktop_spec::parse_desktop_entry(&content))
}

/// Stable MRU key for a desktop launcher: its resolved argv.
///
/// Keying on the argv rather than the raw `Exec` string keeps entries stable
/// across field-code edits, and keying on it rather than the `.desktop` path
/// survives store-path churn on Nix. `build_item` and the execute path must
/// use the same key or scores never accumulate.
///
/// ponytail: reads the file on every desktop launch; fine at CLI cadence.
pub fn desktop_mru_key(path: &Path) -> Option<String> {
    let content = fs::read_to_string(path).ok()?;
    let entry = desktop_spec::parse_desktop_entry(&content);
    let argv = desktop_spec::exec_to_argv(entry.exec.as_deref()?, &entry, Some(path))?;
    Some(argv.join(" "))
}

/// Turn a visible entry into a [`PickerItem`], scoring it against `query`.
fn build_item(
    entry: &DesktopEntry,
    path: &Path,
    query: &str,
    matcher: &SkimMatcherV2,
    mru: &MruCache,
) -> Option<PickerItem> {
    if entry.name.is_empty() {
        return None;
    }

    let exec = entry.exec.as_deref()?;
    // Parse `Exec` per the field-code rules; this is what actually runs.
    let argv = desktop_spec::exec_to_argv(exec, entry, Some(path))?;
    let program = argv.first().cloned().unwrap_or_default();

    // Must match `desktop_mru_key`, which the execute path writes.
    let mru_key = argv.join(" ");
    let boost = (mru.get(&mru_key) as i64) * MRU_BOOST_FACTOR;

    let score = if query.is_empty() {
        boost
    } else if let Some(s) = matcher.fuzzy_match(&entry.name, query) {
        s + boost
    } else if let Some(s) = matcher.fuzzy_match(&program, query) {
        (s - 10) + boost
    } else {
        return None;
    };

    let icon = if entry.icon.is_empty() {
        DEFAULT_ICON.to_string()
    } else {
        entry.icon.clone()
    };

    Some(PickerItem {
        text: entry.name.clone(),
        // The execute token is the absolute `.desktop` path so the launch can be
        // re-resolved without trusting a shell. `text` carries what is shown.
        path: path.to_string_lossy().into_owned(),
        icon,
        is_dir: false,
        is_submenu: false,
        target_source: None,
        score,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_file_id_is_basename() {
        let path = PathBuf::from("/usr/share/applications/firefox.desktop");
        assert_eq!(desktop_file_id(&path).as_deref(), Some("firefox.desktop"));
    }

    #[test]
    fn desktop_file_id_uses_subdirectory_prefix() {
        let path = PathBuf::from("/usr/share/applications/kde/kate.desktop");
        assert_eq!(desktop_file_id(&path).as_deref(), Some("kde-kate.desktop"));
    }

    #[test]
    fn push_unique_preserves_first_occurrence() {
        let mut dirs = vec![PathBuf::from("/a"), PathBuf::from("/b")];
        push_unique(&mut dirs, PathBuf::from("/b"));
        push_unique(&mut dirs, PathBuf::from("/c"));
        assert_eq!(
            dirs,
            vec![
                PathBuf::from("/a"),
                PathBuf::from("/b"),
                PathBuf::from("/c")
            ]
        );
    }

    #[test]
    fn application_dirs_are_unique() {
        let dirs = application_dirs();
        let mut seen = HashSet::new();
        for dir in &dirs {
            assert!(seen.insert(dir.clone()), "duplicate dir: {:?}", dir);
        }
    }

    // ------------------------------------------------------------------
    // Fixture-based indexer tests
    // ------------------------------------------------------------------

    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static FIXTURE_COUNTER: AtomicUsize = AtomicUsize::new(0);

    /// Create an isolated temp directory for a test fixture.
    fn fixture_dir() -> PathBuf {
        let n = FIXTURE_COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("apicker-desktop-test-{}-{}", std::process::id(), n));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create fixture dir");
        dir
    }

    fn write_desktop(dir: &Path, name: &str, content: &str) {
        fs::create_dir_all(dir).expect("create applications dir");
        fs::write(dir.join(name), content).expect("write desktop file");
    }

    fn index(dirs: &[PathBuf], exclude: &[String], desktops: &[&str]) -> Vec<PickerItem> {
        let provider = DesktopProvider {
            exclude,
            desktops: desktops.iter().map(|s| s.to_string()).collect(),
        };
        let mru = MruCache::default();
        let matcher = SkimMatcherV2::default();
        provider.collect(dirs, "", &mru, &matcher)
    }

    fn names(items: &[PickerItem]) -> Vec<String> {
        let mut names: Vec<String> = items.iter().map(|i| i.text.clone()).collect();
        names.sort();
        names
    }

    #[test]
    fn indexer_excludes_nodisplay_entries() {
        let dir = fixture_dir();
        write_desktop(
            &dir,
            "visible.desktop",
            "[Desktop Entry]\nType=Application\nName=Visible\nExec=true\n",
        );
        write_desktop(
            &dir,
            "hidden-url-handler.desktop",
            "[Desktop Entry]\nType=Application\nName=URL Handler\nExec=true\nNoDisplay=true\n",
        );

        let items = index(std::slice::from_ref(&dir), &[], &[]);
        assert_eq!(names(&items), vec!["Visible"]);
    }

    #[test]
    fn indexer_excludes_non_application_and_tryexec() {
        let dir = fixture_dir();
        write_desktop(
            &dir,
            "link.desktop",
            "[Desktop Entry]\nType=Link\nName=A Link\nURL=https://example.com\n",
        );
        write_desktop(
            &dir,
            "missing-tryexec.desktop",
            "[Desktop Entry]\nType=Application\nName=Missing\nExec=true\nTryExec=apicker-no-such-cmd-zzz\n",
        );
        write_desktop(
            &dir,
            "ok.desktop",
            "[Desktop Entry]\nType=Application\nName=Okay\nExec=true\nTryExec=sh\n",
        );

        let items = index(std::slice::from_ref(&dir), &[], &[]);
        assert_eq!(names(&items), vec!["Okay"]);
    }

    #[test]
    fn indexer_respects_only_show_in() {
        let dir = fixture_dir();
        write_desktop(
            &dir,
            "gnome-only.desktop",
            "[Desktop Entry]\nType=Application\nName=GnomeOnly\nExec=true\nOnlyShowIn=GNOME;\n",
        );

        assert!(index(std::slice::from_ref(&dir), &[], &["GNOME"]).len() == 1);
        assert!(index(std::slice::from_ref(&dir), &[], &["river"]).is_empty());
    }

    #[test]
    fn higher_precedence_entry_wins() {
        let high = fixture_dir();
        let low = fixture_dir();
        write_desktop(
            &high,
            "app.desktop",
            "[Desktop Entry]\nType=Application\nName=HighPriority\nExec=true\n",
        );
        write_desktop(
            &low,
            "app.desktop",
            "[Desktop Entry]\nType=Application\nName=LowPriority\nExec=true\n",
        );

        let items = index(&[high.clone(), low.clone()], &[], &[]);
        assert_eq!(names(&items), vec!["HighPriority"]);
    }

    #[test]
    fn higher_precedence_hidden_suppresses_lower_launcher() {
        let high = fixture_dir();
        let low = fixture_dir();
        // A user hides an app that also ships a visible system launcher.
        write_desktop(
            &high,
            "app.desktop",
            "[Desktop Entry]\nType=Application\nName=App\nExec=true\nHidden=true\n",
        );
        write_desktop(
            &low,
            "app.desktop",
            "[Desktop Entry]\nType=Application\nName=App\nExec=true\n",
        );

        let items = index(&[high.clone(), low.clone()], &[], &[]);
        assert!(
            items.is_empty(),
            "hidden override must suppress the system launcher, got {:?}",
            names(&items)
        );
    }

    #[test]
    fn higher_precedence_nodisplay_suppresses_lower_launcher() {
        let high = fixture_dir();
        let low = fixture_dir();
        write_desktop(
            &high,
            "app.desktop",
            "[Desktop Entry]\nType=Application\nName=App\nExec=true\nNoDisplay=true\n",
        );
        write_desktop(
            &low,
            "app.desktop",
            "[Desktop Entry]\nType=Application\nName=App\nExec=true\n",
        );

        let items = index(&[high.clone(), low.clone()], &[], &[]);
        assert!(items.is_empty(), "got {:?}", names(&items));
    }

    #[test]
    fn exclusion_list_hides_by_id() {
        let dir = fixture_dir();
        write_desktop(
            &dir,
            "annoying.desktop",
            "[Desktop Entry]\nType=Application\nName=Annoying\nExec=true\n",
        );
        write_desktop(
            &dir,
            "fine.desktop",
            "[Desktop Entry]\nType=Application\nName=Fine\nExec=true\n",
        );

        let items = index(
            std::slice::from_ref(&dir),
            &["annoying.desktop".to_string()],
            &[],
        );
        assert_eq!(names(&items), vec!["Fine"]);
    }

    #[test]
    fn exclusion_list_matches_id_basename_and_stem() {
        let dir = fixture_dir();
        write_desktop(
            &dir,
            "foo.desktop",
            "[Desktop Entry]\nType=Application\nName=Foo\nExec=true\n",
        );

        // Bare stem (no extension) should match.
        assert!(index(std::slice::from_ref(&dir), &["foo".to_string()], &[]).is_empty());
        // Basename should match, case-insensitively.
        assert!(index(
            std::slice::from_ref(&dir),
            &["FOO.DESKTOP".to_string()],
            &[]
        )
        .is_empty());
        // Unrelated pattern should not match.
        assert_eq!(
            index(std::slice::from_ref(&dir), &["bar".to_string()], &[]).len(),
            1
        );
    }

    #[test]
    fn exec_argv_is_produced_without_shell() {
        let dir = fixture_dir();
        write_desktop(
            &dir,
            "app.desktop",
            "[Desktop Entry]\nType=Application\nName=App\nExec=/usr/bin/app --flag %U\nIcon=app\n",
        );

        let items = index(std::slice::from_ref(&dir), &[], &[]);
        assert_eq!(items.len(), 1);
        // `path` is the re-resolvable desktop-file path, not a shell command.
        assert!(items[0].path.ends_with("app.desktop"));
        // The execute path derives its MRU key from the same parse.
        assert_eq!(
            desktop_mru_key(&dir.join("app.desktop")).as_deref(),
            Some("/usr/bin/app --flag")
        );
    }

    #[test]
    fn mru_boost_uses_the_same_key_the_execute_path_writes() {
        let dir = fixture_dir();
        write_desktop(
            &dir,
            "app.desktop",
            "[Desktop Entry]\nType=Application\nName=App\nExec=/usr/bin/app --flag\n",
        );

        let mut mru = MruCache::default();
        mru.increment(&desktop_mru_key(&dir.join("app.desktop")).unwrap());

        let provider = DesktopProvider {
            exclude: &[],
            desktops: Vec::new(),
        };
        let items = provider.collect(
            std::slice::from_ref(&dir),
            "App",
            &mru,
            &SkimMatcherV2::default(),
        );
        assert_eq!(items.len(), 1);
        assert!(
            items[0].score >= MRU_BOOST_FACTOR,
            "expected MRU boost, got score {}",
            items[0].score
        );
    }
}
