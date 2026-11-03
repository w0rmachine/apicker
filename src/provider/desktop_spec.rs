//! XDG Desktop Entry specification logic.
//!
//! This module implements the pieces of the
//! [Desktop Entry Specification](https://specifications.freedesktop.org/desktop-entry-spec/latest/)
//! that determine whether a launcher should be shown to the user, and how its
//! `Exec` key should be turned into an argument vector.
//!
//! Everything here is pure (no filesystem discovery, no globals) so it can be
//! unit tested in isolation.

use std::collections::HashMap;
use std::path::Path;

/// A single parsed `[Desktop Entry]` group.
///
/// Only the keys relevant to visibility, launching and de-duplication are kept.
/// Keys are stored verbatim (after unescaping) in `values` as well so callers
/// can look up less common keys (e.g. `Name[de]` fallbacks) if they want.
#[derive(Debug, Clone, Default)]
pub struct DesktopEntry {
    /// Raw `Type=` value, if present.
    pub type_: Option<String>,
    /// Raw `Hidden=` value parsed as a boolean.
    pub hidden: bool,
    /// Raw `NoDisplay=` value parsed as a boolean.
    pub no_display: bool,
    /// Raw `TryExec=` value, if present.
    pub try_exec: Option<String>,
    /// `OnlyShowIn=` split on `;`, empty when the key is absent.
    pub only_show_in: Vec<String>,
    /// `NotShowIn=` split on `;`, empty when the key is absent.
    pub not_show_in: Vec<String>,
    /// Localized `Name` chosen at parse time.
    pub name: String,
    /// Raw `Icon=` value.
    pub icon: String,
    /// Raw `Exec=` value (field codes not yet expanded).
    pub exec: Option<String>,
    /// Every `Key=Value` pair seen in `[Desktop Entry]` (last wins), used for
    /// localized lookups and diagnostics.
    pub values: HashMap<String, String>,
}

impl DesktopEntry {
    /// The `Type` key, if any.
    pub fn type_(&self) -> Option<&str> {
        self.type_.as_deref()
    }

    /// Whether the entry is `Hidden=true`.
    pub fn hidden(&self) -> bool {
        self.hidden
    }

    /// Whether the entry is `NoDisplay=true`.
    pub fn no_display(&self) -> bool {
        self.no_display
    }

    /// The `TryExec` value, if any.
    pub fn try_exec(&self) -> Option<&str> {
        self.try_exec.as_deref()
    }

    /// Evaluate `OnlyShowIn` / `NotShowIn` against the active desktop names.
    ///
    /// `desktops` is the list of desktop names currently in effect, normally
    /// derived from `$XDG_CURRENT_DESKTOP` (a `:`-separated list). Matching is
    /// case-insensitive and follows the spec:
    ///
    /// * If `OnlyShowIn` is present and none of `desktops` appears in it, the
    ///   entry must not be shown.
    /// * If `NotShowIn` is present and any of `desktops` appears in it, the
    ///   entry must not be shown.
    /// * An absent key imposes no restriction.
    ///
    /// A `desktops` list that is empty (no `XDG_CURRENT_DESKTOP`) imposes no
    /// `OnlyShowIn` restriction — we cannot prove the entry is inapplicable.
    pub fn matches_desktop(&self, desktops: &[&str]) -> bool {
        if !self.only_show_in.is_empty() {
            if desktops.is_empty() {
                // No active desktop to compare against; do not hide entries
                // that restrict themselves to a desktop we cannot name.
                return true;
            }
            let allowed = desktops.iter().any(|d| {
                self.only_show_in
                    .iter()
                    .any(|only| only.eq_ignore_ascii_case(d))
            });
            if !allowed {
                return false;
            }
        }

        if !self.not_show_in.is_empty() {
            let denied = desktops.iter().any(|d| {
                self.not_show_in
                    .iter()
                    .any(|not| not.eq_ignore_ascii_case(d))
            });
            if denied {
                return false;
            }
        }

        true
    }
}

/// Decides whether a [`DesktopEntry`] is a visible application launcher.
///
/// This mirrors the specification's visibility rules and is the single
/// authority used by the indexer:
///
/// * `Type` must be `Application`.
/// * `Hidden=true` hides the entry.
/// * `NoDisplay=true` hides the entry.
/// * A set `TryExec` must name an executable that exists on `$PATH` (or be an
///   existing absolute path).
/// * `OnlyShowIn` / `NotShowIn` must accept one of `desktops`.
///
/// `command_exists` is injected so the check can be tested without a real
/// filesystem or `$PATH`.
pub fn is_visible(entry: &DesktopEntry, desktops: &[&str]) -> bool {
    is_visible_with(entry, desktops, command_exists)
}

/// Like [`is_visible`] but with an injectable executable resolver.
pub fn is_visible_with(
    entry: &DesktopEntry,
    desktops: &[&str],
    command_exists: impl Fn(&str) -> bool,
) -> bool {
    entry.type_() == Some("Application")
        && !entry.hidden()
        && !entry.no_display()
        && entry.try_exec().is_none_or(&command_exists)
        && entry.matches_desktop(desktops)
}

/// Returns `true` when `command` resolves to an executable.
///
/// An absolute or slash-containing `TryExec` is checked directly on the
/// filesystem; a bare name is looked up in each `$PATH` component.
pub fn command_exists(command: &str) -> bool {
    let command = command.trim();
    if command.is_empty() {
        return false;
    }

    if command.contains('/') {
        return is_executable_file(Path::new(command));
    }

    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };

    std::env::split_paths(&path).any(|dir| {
        let candidate = dir.join(command);
        is_executable_file(&candidate)
    })
}

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::metadata(path) {
        Ok(meta) => meta.is_file() && meta.permissions().mode() & 0o111 != 0,
        Err(_) => false,
    }
}

#[cfg(not(unix))]
fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

/// Parse the contents of a `.desktop` file into a [`DesktopEntry`].
///
/// Only the `[Desktop Entry]` group is considered, per the specification; any
/// later group (`[Desktop Action ...]`, ...) is ignored. Malformed lines are
/// skipped rather than aborting the parse.
pub fn parse_desktop_entry(content: &str) -> DesktopEntry {
    let mut entry = DesktopEntry::default();
    let mut in_desktop_entry = false;
    let mut in_other_group = false;

    // Localized names are ranked so we can prefer an exact language match over
    // a regional variant, mirroring the spec's `Name[lang_COUNTRY]` rules.
    let mut best_name_rank: Option<u8> = None;
    let locales = current_locales();

    for raw_line in content.lines() {
        let line = raw_line.trim();

        if line.starts_with('[') {
            if line == "[Desktop Entry]" {
                in_desktop_entry = true;
                in_other_group = false;
            } else {
                in_desktop_entry = false;
                in_other_group = true;
            }
            continue;
        }

        if !in_desktop_entry || in_other_group {
            continue;
        }

        // A line without '=' is not a valid entry; ignore it.
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();

        entry.values.insert(key.to_string(), value.to_string());

        match key {
            "Type" => entry.type_ = Some(value.to_string()),
            "Hidden" => entry.hidden = parse_bool(value),
            "NoDisplay" => entry.no_display = parse_bool(value),
            "TryExec" => entry.try_exec = Some(unescape_string(value)),
            "OnlyShowIn" => entry.only_show_in = split_semicolon(value),
            "NotShowIn" => entry.not_show_in = split_semicolon(value),
            "Icon" => {
                if entry.icon.is_empty() {
                    entry.icon = value.to_string();
                }
            }
            "Exec" => {
                if entry.exec.is_none() {
                    entry.exec = Some(value.to_string());
                }
            }
            _ => {
                if let Some(locale) = key.strip_prefix("Name[") {
                    let locale = locale.trim_end_matches(']');
                    if let Some(rank) = locale_rank(locale, &locales) {
                        if best_name_rank.is_none_or(|best| rank > best) {
                            best_name_rank = Some(rank);
                            entry.name = unescape_string(value);
                        }
                    }
                } else if key == "Name" && entry.name.is_empty() {
                    // Only used if no localized name wins.
                    entry.name = unescape_string(value);
                }
            }
        }
    }

    entry
}

fn parse_bool(value: &str) -> bool {
    value.eq_ignore_ascii_case("true")
}

/// Split a `;`-separated list, dropping empty segments (the spec permits a
/// trailing `;`).
fn split_semicolon(value: &str) -> Vec<String> {
    value
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .collect()
}

/// Unescape the `\s`, `\n`, `\t`, `\r` and `\\` sequences the spec defines for
/// string values.
pub fn unescape_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                // Unknown escape: keep the backslash, per the spec.
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// Expand the `Exec` key into an argv vector.
///
/// The parser is a strict implementation of the spec's quoting and
/// field-code rules:
///
/// * Arguments are separated by unquoted whitespace.
/// * `"..."` preserves whitespace; `\\`, `\"`, `` \` ``, `\$` are unescaped
///   inside quotes.
/// * Field codes that represent a file/URL argument (`%f`, `%F`, `%u`, `%U`)
///   expand to nothing, because apicker launches applications without a
///   document argument. If a caller ever has a URL it can be substituted here.
/// * `%c` expands to the localized application name.
/// * `%k` expands to the desktop file path.
/// * `%i` expands to `--icon <icon>` when an icon is known, otherwise nothing.
/// * `%%` is a literal `%`.
/// * Unknown codes are dropped. A code at the end of an argument (`foo%f`)
///   contributes nothing, matching the "expand in place" rule.
///
/// Returns `None` when no executable token remains.
pub fn exec_to_argv(
    exec: &str,
    entry: &DesktopEntry,
    desktop_path: Option<&Path>,
) -> Option<Vec<String>> {
    let tokens = tokenize_exec(exec);

    let mut argv: Vec<String> = Vec::new();
    for token in tokens {
        let mut expanded = String::new();
        // `%i` is special: it expands to *two* arguments when an icon exists.
        let mut icon_args: Vec<String> = Vec::new();

        let mut chars = token.chars().peekable();
        while let Some(c) = chars.next() {
            if c != '%' {
                expanded.push(c);
                continue;
            }

            let Some(code) = chars.next() else {
                // Trailing lone '%' — drop it.
                break;
            };

            match code {
                '%' => expanded.push('%'),
                // File / URL codes: no document is being opened.
                'f' | 'F' | 'u' | 'U' => {}
                'c' => expanded.push_str(&entry.name),
                'k' => {
                    if let Some(path) = desktop_path {
                        expanded.push_str(&path.to_string_lossy());
                    }
                }
                'i' if !entry.icon.is_empty() => {
                    icon_args.push("--icon".to_string());
                    icon_args.push(entry.icon.clone());
                }
                // Unknown field code: drop it.
                _ => {}
            }
        }

        if expanded.is_empty() && icon_args.is_empty() {
            continue;
        }

        if !expanded.is_empty() {
            argv.push(expanded);
        }
        argv.extend(icon_args);
    }

    if argv.is_empty() {
        None
    } else {
        Some(argv)
    }
}

/// Split an `Exec` string into raw argument tokens, honoring quoting rules.
fn tokenize_exec(exec: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut has_token = false;

    let mut chars = exec.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                in_quotes = !in_quotes;
                has_token = true;
            }
            '\\' if in_quotes => {
                // Inside quotes only these escapes are meaningful.
                match chars.next() {
                    Some(next @ ('"' | '`' | '$' | '\\')) => {
                        current.push(next);
                        has_token = true;
                    }
                    Some(other) => {
                        current.push('\\');
                        current.push(other);
                        has_token = true;
                    }
                    None => current.push('\\'),
                }
            }
            c if c.is_whitespace() && !in_quotes => {
                if has_token {
                    tokens.push(std::mem::take(&mut current));
                    has_token = false;
                }
            }
            c => {
                current.push(c);
                has_token = true;
            }
        }
    }

    if has_token {
        tokens.push(current);
    }

    tokens
}

/// Locales that a `Name[locale]` key should be matched against, most specific
/// first (e.g. `de_DE`, `de`).
fn current_locales() -> Vec<String> {
    for key in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(value) = std::env::var(key) {
            if value.is_empty() || value == "C" || value == "POSIX" {
                continue;
            }
            let value = value.split('.').next().unwrap_or(&value);
            let value = value.split('@').next().unwrap_or(value);
            let mut locales = Vec::new();
            if let Some((lang, country)) = value.split_once('_') {
                locales.push(format!("{lang}_{country}"));
                locales.push(lang.to_string());
            } else {
                locales.push(value.to_string());
            }
            return locales;
        }
    }
    Vec::new()
}

/// Rank a `Name[locale]` candidate: higher is better. `None` means it does not
/// apply to any active locale.
fn locale_rank(locale: &str, locales: &[String]) -> Option<u8> {
    locales
        .iter()
        .position(|active| active.eq_ignore_ascii_case(locale))
        .map(|index| (locales.len() - index) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn entry_with(content: &str) -> DesktopEntry {
        parse_desktop_entry(content)
    }

    #[test]
    fn parses_basic_application() {
        let entry = entry_with(
            "[Desktop Entry]\nType=Application\nName=Firefox\nExec=firefox %u\nIcon=firefox\n",
        );
        assert_eq!(entry.type_(), Some("Application"));
        assert_eq!(entry.name, "Firefox");
        assert_eq!(entry.icon, "firefox");
        assert_eq!(entry.exec.as_deref(), Some("firefox %u"));
        assert!(is_visible(&entry, &[]));
    }

    #[test]
    fn ignores_keys_outside_desktop_entry_group() {
        let entry = entry_with(
            "[Desktop Entry]\nType=Application\nName=Firefox\nExec=firefox\n\n[Desktop Action new-window]\nName=New Window\nExec=firefox --new-window\n",
        );
        // The action group's Name/Exec must not override the main entry.
        assert_eq!(entry.name, "Firefox");
        assert_eq!(entry.exec.as_deref(), Some("firefox"));
    }

    #[test]
    fn hidden_and_nodisplay_are_excluded() {
        let hidden = entry_with("[Desktop Entry]\nType=Application\nName=X\nExec=x\nHidden=true\n");
        assert!(!is_visible(&hidden, &[]));

        let nodisplay =
            entry_with("[Desktop Entry]\nType=Application\nName=X\nExec=x\nNoDisplay=true\n");
        assert!(!is_visible(&nodisplay, &[]));
    }

    #[test]
    fn non_application_type_is_excluded() {
        let link = entry_with("[Desktop Entry]\nType=Link\nName=X\nURL=https://example.com\n");
        assert!(!is_visible(&link, &[]));
    }

    #[test]
    fn try_exec_missing_command_is_excluded() {
        let entry = entry_with("[Desktop Entry]\nType=Application\nName=X\nExec=x\nTryExec=definitely-not-a-real-cmd-xyz\n");
        assert!(!is_visible(&entry, &[]));
    }

    #[test]
    fn try_exec_present_command_is_shown() {
        let entry = entry_with("[Desktop Entry]\nType=Application\nName=X\nExec=x\nTryExec=sh\n");
        assert!(is_visible(&entry, &[]));
        // Injectable resolver proves the branch is the deciding factor.
        assert!(is_visible_with(&entry, &[], |_| true));
        assert!(!is_visible_with(&entry, &[], |_| false));
    }

    #[test]
    fn only_show_in_filters_other_desktops() {
        let entry = entry_with(
            "[Desktop Entry]\nType=Application\nName=X\nExec=x\nOnlyShowIn=GNOME;Unity;\n",
        );
        assert!(entry.matches_desktop(&["GNOME"]));
        assert!(entry.matches_desktop(&["river", "GNOME"]));
        assert!(!entry.matches_desktop(&["river"]));
        assert!(
            entry.matches_desktop(&[]),
            "no active desktop should not hide"
        );
    }

    #[test]
    fn not_show_in_filters_listed_desktop() {
        let entry =
            entry_with("[Desktop Entry]\nType=Application\nName=X\nExec=x\nNotShowIn=KDE;GNOME;\n");
        assert!(!entry.matches_desktop(&["KDE"]));
        assert!(!entry.matches_desktop(&["river", "GNOME"]));
        assert!(entry.matches_desktop(&["river"]));
    }

    #[test]
    fn desktop_matching_is_case_insensitive() {
        let entry =
            entry_with("[Desktop Entry]\nType=Application\nName=X\nExec=x\nOnlyShowIn=gnome;\n");
        assert!(entry.matches_desktop(&["GNOME"]));
        let entry =
            entry_with("[Desktop Entry]\nType=Application\nName=X\nExec=x\nNotShowIn=gnome;\n");
        assert!(!entry.matches_desktop(&["GNOME"]));
    }

    #[test]
    fn exec_simple_tokens() {
        let entry = entry_with("[Desktop Entry]\nType=Application\nName=X\nExec=btop\n");
        assert_eq!(exec_to_argv("btop", &entry, None).unwrap(), vec!["btop"]);
    }

    #[test]
    fn exec_drops_field_codes() {
        let entry = entry_with(
            "[Desktop Entry]\nType=Application\nName=X\nExec=obsidian %u\nIcon=obsidian\n",
        );
        assert_eq!(
            exec_to_argv("obsidian %u", &entry, None).unwrap(),
            vec!["obsidian"]
        );
        assert_eq!(exec_to_argv("nvim %F", &entry, None).unwrap(), vec!["nvim"]);
    }

    #[test]
    fn exec_preserves_quoted_arguments() {
        let entry = entry_with(
            "[Desktop Entry]\nType=Application\nName=X\nExec=\"my app\" --flag \"a b\"\n",
        );
        assert_eq!(
            exec_to_argv(r#""my app" --flag "a b""#, &entry, None).unwrap(),
            vec!["my app", "--flag", "a b"]
        );
    }

    #[test]
    fn exec_handles_escapes_inside_quotes() {
        let entry =
            entry_with("[Desktop Entry]\nType=Application\nName=X\nExec=cmd \"a \\\"b\\\" c\"\n");
        assert_eq!(
            exec_to_argv(r#"cmd "a \"b\" c""#, &entry, None).unwrap(),
            vec!["cmd", "a \"b\" c"]
        );
    }

    #[test]
    fn exec_expands_name_icon_and_path() {
        let entry = entry_with(
            "[Desktop Entry]\nType=Application\nName=My App\nExec=cmd %c\nIcon=myicon\n",
        );
        let path = PathBuf::from("/usr/share/applications/myapp.desktop");
        assert_eq!(
            exec_to_argv("cmd %c", &entry, Some(&path)).unwrap(),
            vec!["cmd", "My App"]
        );
        assert_eq!(
            exec_to_argv("cmd %i", &entry, Some(&path)).unwrap(),
            vec!["cmd", "--icon", "myicon"]
        );
        assert_eq!(
            exec_to_argv("cmd %k", &entry, Some(&path)).unwrap(),
            vec!["cmd", "/usr/share/applications/myapp.desktop"]
        );
    }

    #[test]
    fn exec_literal_percent() {
        let entry = entry_with("[Desktop Entry]\nType=Application\nName=X\nExec=cmd 100%%\n");
        assert_eq!(
            exec_to_argv("cmd 100%%", &entry, None).unwrap(),
            vec!["cmd", "100%"]
        );
    }

    #[test]
    fn exec_env_prefix_is_preserved_as_argv() {
        let entry = entry_with("[Desktop Entry]\nType=Application\nName=X\nExec=env NO_AT_BRIDGE=1 /bin/orca-slicer %U\n");
        assert_eq!(
            exec_to_argv("env NO_AT_BRIDGE=1 /bin/orca-slicer %U", &entry, None).unwrap(),
            vec!["env", "NO_AT_BRIDGE=1", "/bin/orca-slicer"]
        );
    }

    #[test]
    fn exec_with_only_field_codes_yields_none() {
        let entry = entry_with("[Desktop Entry]\nType=Application\nName=X\nExec=%F\n");
        assert!(exec_to_argv("%F", &entry, None).is_none());
    }

    #[test]
    fn unescapes_string_values() {
        assert_eq!(unescape_string(r"a\sb\nc\\d"), "a b\nc\\d");
    }
}
