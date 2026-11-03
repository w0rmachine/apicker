# apicker

`apicker` is a lightning-fast, scriptable backend written in Rust, designed to power frontend launcher interfaces like [Quickshell](https://git.outfoxxed.me/outfoxxed/quickshell). 

Instead of dealing with the complexities of Wayland layer-shells and UI rendering in Rust, `apicker` focuses entirely on data: **fast fuzzy matching**, **MRU caching**, **.desktop file parsing**, and **custom shell script sources**. It outputs strict JSON that your QML frontend can easily consume and animate.

---

## 🚀 Key Features

- **Blazing Fast File Search:** Uses asynchronous `WalkDir` caching and `fuzzy-matcher` to instantly find files.
- **Native Desktop Apps:** Automatically discovers and parses `.desktop` entries from `$XDG_DATA_HOME` and `$XDG_DATA_DIRS`, honoring the [Desktop Entry Specification](https://specifications.freedesktop.org/desktop-entry-spec/latest/): non-`Application` types, `Hidden`/`NoDisplay` entries, missing `TryExec` commands, and desktop-restricted (`OnlyShowIn`/`NotShowIn`) entries are excluded automatically.
- **Intelligent MRU (Most Recently Used):** Automatically tracks the files and apps you open. The more you use something, the higher it gets boosted in the search results.
- **Nix Native:** Exposes a declarative Home Manager module.
- **Submenus & Custom Scripts:** Easily create submenus for clipboards, power options, or bookmarks using simple shell scripts.

---

## 🖥️ Desktop Entries (XDG compliant)

The `desktop` source indexes `.desktop` files following the Desktop Entry
Specification rather than scanning blindly:

* **Directory precedence:** `$XDG_DATA_HOME/applications` (usually
  `~/.local/share/applications`) wins over each entry in `$XDG_DATA_DIRS`, so a
  Nix profile or `/usr/share` launcher is overridden by a user copy.
* **De-duplication by desktop-file ID:** duplicates are resolved by ID
  (`foo.desktop`, `kde-kate.desktop`, ...), not by whichever directory happened
  to be scanned first. A higher-priority `Hidden=true`/`NoDisplay=true` entry
  suppresses the lower-priority launcher completely.
* **Visibility rules:** an entry is excluded when
  `Type != Application`, `Hidden=true`, `NoDisplay=true`, `TryExec` names a
  command that is not on `$PATH`, or `OnlyShowIn`/`NotShowIn` excludes the
  active `$XDG_CURRENT_DESKTOP`. This hides portal/polkit/IBus/snap-agent and
  GNOME service entries without any distro-specific blacklist.
* **`Exec` field codes:** `Exec` is parsed per spec into an argv vector
  (`%f`/`%F`/`%u`/`%U` are dropped, `%c` becomes the app name, `%k` the desktop
  file path, `%i` expands to `--icon <name>`, `%%` is a literal `%`). Launches
  are spawned directly, **never through `sh -c`**, so `Exec` values cannot
  inject shell commands.

If an entry is genuinely mislabeled upstream, add it to `desktop_exclude` in
`~/.config/apicker/config.toml` as an escape hatch:

```toml
# Matches by desktop-file ID, basename, or basename without .desktop.
desktop_exclude = ["some-broken-launcher.desktop"]
```

---

## 🛠️ Configuration (Nix Flake)

`apicker` is designed to be configured declaratively via Nix. Add the flake to your dotfiles:

```nix
inputs.apicker.url = "github:yourusername/apicker";
```

Then configure the Home Manager module:

```nix
imports = [
  inputs.apicker.homeManagerModules.default
];

programs.apicker = {
  enable = true;
  
  # Optimize file searching by ignoring heavy directories
  ignore_dirs = [ ".venv" "node_modules" "target" ".git" ];

  # Escape hatch only: hide a specific mislabeled launcher by desktop-file ID.
  # Normal visibility follows the XDG Desktop Entry specification.
  desktop_exclude = [ "some-broken-launcher.desktop" ];
  
  settings = {
    sources = [
      {
        name = "Power Options";
        icon = "system-shutdown";
        description = "Shutdown, Reboot, Logout";
        script = ''
          echo "Shutdown"
          echo "Reboot"
          echo "Logout"
        '';
        # `{item}` is substituted as a single-quoted shell literal. Use it bare;
        # wrapping it in quotes makes those quotes part of the value.
        action = ''
          case {item} in
            Shutdown) systemctl poweroff ;;
            Reboot) systemctl reboot ;;
            Logout) hyprctl dispatch exit ;;
          esac
        '';
      },
      {
        name = "Clipboard";
        icon = "edit-copy";
        description = "Clipboard History";
        # Assuming you use cliphist
        script = "cliphist list";
        action = "echo '{item}' | cliphist decode | wl-copy";
      }
    ];
  };
};
```

---

## 💡 Quickshell Use Cases

Because `apicker` is headless, it perfectly pairs with your Quickshell dotfiles to create beautiful, animated, and highly functional widgets.

### 1. The Dynamic Island App Launcher (`Super + D`)
Design a QML "Dynamic Island" that expands into a list when you press `Super + D`.
* **The Call:** Your QML runs `apicker search --source desktop --query "<input>"`
* **The Result:** It strictly searches your `.desktop` applications. Because of the built-in MRU cache, apps you use daily (like Firefox or your terminal) will always instantly appear at the top. QML handles the smooth expanding animations.

### 2. Fuzzy File Finder with Previews (`Super + F`)
Design a large popup window for deep file system traversal.
* **The Call:** Your QML runs `apicker search --source files --query "<input>"`
* **Previews:** In your QML `TextField`, listen for the `v` key (`Keys.onPressed`). If pressed, grab the `path` of the highlighted JSON result. If it's an image, render it in a QML `Image` component. If it's text, run `bat --color=always` and display the syntax-highlighted output in a side-panel.

For exact file-picking patterns, pass a root and glob:

```sh
apicker search --source files --root / --glob '*.backup' --query ''
```

By default, file searches use the cached index for speed. Add `--no-cache` when you need a live filesystem scan:

```sh
apicker search --source files --root / --glob '*.backup' --no-cache
```

Glob patterns without `/` match file names anywhere below the root. Patterns containing `/` match the full path.

An empty query without `--glob` is a *recent files* view: only entries with MRU
history are returned, so the picker does not surface an arbitrary slice of the
whole index. Passing `--glob` lists every match regardless of recent use.

File searches skip dotfiles and dot-directories by default. Pass `--hidden` (or set
`include_hidden = true` in `config.toml`) to index them, so `~/.config/nvim/init.lua`
or `.env` become findable:

```sh
apicker search --source files --root ~ --hidden --query env
```

The flag also applies to `apicker rebuild-cache --hidden`; a cache built with hidden
files is not reused for a normal search and vice-versa.

### Calling `execute`

Pass the result's `source` together with its `path`; omit the source for
desktop or file items. `{item}` in a custom `action` is passed to the shell
as a single argument, so `echo '{item}' | wl-copy` copies exactly the picked
text, even when it contains quotes or `$`.

```sh
apicker execute --source ssh --item home-server
apicker execute --source desktop --item ~/.local/share/applications/foo.desktop
```

### 3. The Master Menu (`Super + M`)
Bind `Super + M` to open a main launcher menu.
* **The Call:** Your QML runs `apicker search --query ":"` (or an empty string).
* **The Result:** This will list all your configured custom sources (e.g., Power Options, Clipboard, Apps). When you select a source, update the QML state to dive into that submenu, seamlessly blending built-in apps and custom scripts into one cohesive interface.
