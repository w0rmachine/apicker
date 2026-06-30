# `apicker` Design & Considerations

This document details the architectural decisions, design patterns, and engineering considerations for building a fast, scriptable file picker in Rust integrated with Quickshell.

---

## 1. Architecture: Rust Backend + Quickshell QML Frontend (Chosen)

To support the goal of a **Dynamic Island** that smoothly expands into a file picker, we are using a split architecture:

In this architecture, the Rust binary (`apicker`) acts as a highly efficient headless command-line interface or background daemon. It handles filesystem indexing, caching, fuzzy filtering, and executing scripts. The visual layout, keyboard grab, smooth expanding animations (for the dynamic island), and input fields are built entirely in QML running natively inside Quickshell.

* **How it works:**
  - Quickshell spawns `apicker` via `Quickshell.Io.Process` (or communicates with a running daemon via a Unix domain socket).
  - When the user interacts with the Dynamic Island or types in the QML input field, Quickshell sends the query to `apicker`.
  - `apicker` fuzzy-matches files/entries and returns a JSON list of matches instantaneously.
  - QML handles the smooth animation from the "Island" state to the expanded popup window (`PopupWindow`), renders the items, handles up/down navigation, and displays icons.
  - Selecting an item triggers `apicker` to run the associated action script or open the file.

* **Why this is the most efficient approach:**
  - **Fluid Animations:** Quickshell handles Wayland layer-shell protocol and animations natively in QtQuick/QML. Building an expanding dynamic island in pure Rust (e.g., with Iced or Egui) and animating layer-shell margins/sizes smoothly is notoriously difficult and prone to visual stutter. QML handles these state transitions effortlessly.
  - **Performance:** Rust does all the heavy lifting (caching, walking directories, fuzzy matching) on a separate thread/process. The UI thread in QML never blocks.
  - **Aesthetics & Theming:** Style changes, blur, and borders can perfectly match your other Quickshell widgets.

---

## 2. Nix Flake: Nix OS vs. Non-Nix Environments

To ensure `apicker` is easily buildable and runnable for both NixOS users and standard Linux/macOS users:

### For Nix / NixOS Users
- **`flake.nix`**: Defines the project packages (`apicker`), dev shells, and overlays.
- **Packaging the QML components**: The Nix package should install both the Rust binary and any default QML scripts (e.g., to `$out/bin/apicker` and `$out/share/apicker/qml/`).
- **Reproducible Toolchain**: Uses `rust-overlay` to lock the specific version of Rust (e.g., stable `1.80+`) and cargo tools, ensuring everyone compiles with the exact same compiler version.

### For Non-Nix Users
- **Standard Cargo Toolchain**: The code should not rely on Nix-specific environment variables or structures. Running `cargo build --release` should output a standalone executable.
- **Assets/Scripts Paths**: The application must search for QML/configuration files in standard XDG directories:
  1. Local config: `~/.config/apicker/`
  2. System-wide data: `/usr/share/apicker/` or `/usr/local/share/apicker/`
  3. Nix store fallback: `/nix/store/.../share/apicker/` (resolved during Nix build phase).

---

## 3. Caching Strategy for Ultra-Fast Search

To keep keyboard interaction instantaneous (<10ms UI updates), `apicker` needs a robust caching strategy:

1. **Pre-populated Cache File**:
   - Save the indexed file tree to a fast binary format or a simple newline-delimited text/JSON file in `~/.cache/apicker/cache.db`.
2. **Background Incremental Indexing**:
   - Instead of blocking the picker startup, read the cache immediately on startup.
   - Run a low-priority background thread to walk directories (using the `ignore` or `walkdir` crate) to update the cache.
   - Use `notify` crate to set up an inotify/Wayland-friendly filesystem watcher for directories that change frequently (like `~/Downloads` or `~/Projects`).
3. **Rust Fuzzy Matcher**:
   - Do the scoring in Rust using algorithms like `nucleo-matcher` or standard Smith-Waterman/fuzzy match.
   - Cache the scoring results of previous keystrokes so that appending characters to a search query only filters the already matched subset rather than re-scoring the entire filesystem.

---

## 4. Scripting Interface & Extensibility

`apicker` should be highly customizable via scripts:

### A. Source Scripts (Generating Items)
Users can register custom scripts to populate the picker instead of files.
* **Inputs**: Search query, parent menu context.
* **Outputs**: JSON to stdout representing items.
* **Example**: A script that fetches browser bookmarks, SSH hosts, clipboard history, or pass passwords.
```json
[
  {"text": "Connect to Home Server", "icon": "network-server", "action": "ssh user@home.local"},
  {"text": "Copy Clipboard Item 1", "icon": "edit-copy", "action": "wl-copy 'item1'"}
]
```

### B. Action Scripts (Handling Selections)
Define what happens when an item is chosen.
* By default, selecting a file opens it with `xdg-open` or a configured default.
* Custom action rules can match file patterns (e.g., `.png` files have custom options like "Edit with GIMP", "Upload to Imgur").

---

## 5. Submenus & Nested Filtering

To support deep navigation (like browsing directory trees or command trees):

- **State Stack**: The picker maintains a stack of active directories or menus (e.g., `["Root", "Projects", "apicker"]`).
- **Input Filtering**:
  - Typing filters the current menu level.
  - Backspace on an empty input pops the stack (moves to parent directory/menu).
  - Pressing `Enter` on a directory or a menu item pushes it to the stack and clears the input, showing the submenu contents instantly.
- **Visual Breadcrumbs**: The UI should display the active breadcrumb path (e.g., `Home > Projects > apicker`) so the user maintains spatial awareness.

---

## 6. Icons Support

Using standard system icons improves visual aesthetics significantly:

- **FreeDesktop Icon Spec**: Rust or QML should read the active GTK/QT system icon theme (e.g. Papirus, Adwaita) and resolve icons (like `folder`, `text-x-generic`, `video-x-generic`) to absolute paths.
- **QML `IconImage`**: QtQuick has built-in support for resolving system theme icons via standard names. If we use the Quickshell approach, QML can resolve icons natively by specifying their standard names (e.g., `icon.name: "folder"`), which simplifies Rust's job to just returning the standard icon name (like `"image-x-generic"`).
