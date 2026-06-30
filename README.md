# apicker

`apicker` is a lightning-fast, scriptable backend written in Rust, designed to power frontend launcher interfaces like [Quickshell](https://git.outfoxxed.me/outfoxxed/quickshell). 

Instead of dealing with the complexities of Wayland layer-shells and UI rendering in Rust, `apicker` focuses entirely on data: **fast fuzzy matching**, **MRU caching**, **.desktop file parsing**, and **custom shell script sources**. It outputs strict JSON that your QML frontend can easily consume and animate.

---

## 🚀 Key Features

- **Blazing Fast File Search:** Uses asynchronous `WalkDir` caching and `fuzzy-matcher` to instantly find files.
- **Native Desktop Apps:** Automatically discovers and parses `.desktop` entries from `/usr/share/applications` and `~/.local/share/applications`, extracting accurate icons and execution strings.
- **Intelligent MRU (Most Recently Used):** Automatically tracks the files and apps you open. The more you use something, the higher it gets boosted in the search results.
- **Nix Native:** Exposes a declarative Home Manager module.
- **Submenus & Custom Scripts:** Easily create submenus for clipboards, power options, or bookmarks using simple shell scripts.

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
        action = ''
          case "{item}" in
            "Shutdown") systemctl poweroff ;;
            "Reboot") systemctl reboot ;;
            "Logout") hyprctl dispatch exit ;;
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

### 3. The Master Menu (`Super + M`)
Bind `Super + M` to open a main launcher menu.
* **The Call:** Your QML runs `apicker search --query ":"` (or an empty string).
* **The Result:** This will list all your configured custom sources (e.g., Power Options, Clipboard, Apps). When you select a source, update the QML state to dive into that submenu, seamlessly blending built-in apps and custom scripts into one cohesive interface.
