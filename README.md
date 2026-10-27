# Space Hog Inspector

A native macOS app, written in Rust, that shows where your disk space goes.
It is similar to SpaceSniffer: a scan draws your folders as a treemap, a set
of nested boxes where each box's area is how much disk space it takes up.

## Look

The interface uses a Neo-Noir Cyberpunk style: an always-dark palette with
cyan, magenta and yellow neon accents, cut-corner panels, and three bundled
typefaces (Chakra Petch for headings, Inter for text, JetBrains Mono for
numbers and paths).

## Features

- **Fast parallel scan** on all CPU cores. The treemap fills in as each
  top-level folder finishes, and you can stop a scan at any time.
- **Real disk usage**: sizes count the space files actually take up on disk,
  not just their listed length. Hard-linked files count once, symlinks aren't
  followed, and APFS firmlinks don't count `/Users` twice when you scan the
  whole disk.
- **Treemap view**: folders are framed boxes with a name and size header.
  Files are coloured by type (video, audio, image, archive, document, code,
  apps and bundles, system). Hover for the full path and size. Double-click to
  zoom in. Use the breadcrumb bar, **Up**, Backspace or Cmd+↑ to go back up.
  A slider sets how many nested levels are drawn.
- **Side panel**: the current folder's contents sorted by size, or the 300
  **largest files** anywhere below it, with a name filter.
- **Actions** (right-click a box or a list row): Zoom In, Reveal in Finder,
  Open, Copy Path, **Ask AI…** (see below), and **Move to Trash…**, which always asks first. Nothing is
  ever deleted permanently: you can restore items from the Trash in Finder.
- **Status bar**: the scan total, how long it took, the volume's free and
  total space, and a list of items that were skipped or that macOS wouldn't
  let it read.
- **Settings** (menu bar **Space Hog Inspector → Settings…**, `Cmd+,`, or the
  toolbar button): Full Disk Access status and setup, whether to skip folders
  that trigger macOS permission prompts, scan defaults, and the AI agent. Settings are saved
  to `~/Library/Application Support/Space Hog Inspector/settings.json`.

Keyboard: `Cmd+O` choose a folder · `Cmd+R` rescan · `Cmd+,` settings ·
`Backspace` / `Cmd+↑` go up a level.

## Build

You need the Rust toolchain (`brew install rustup && rustup default stable`;
the Homebrew rustup is keg-only, so add `/opt/homebrew/opt/rustup/bin` to your
`PATH`) and the Xcode command-line tools.

```bash
packaging/bundle.sh
```

This builds an optimised binary and produces
`target/release/Space Hog Inspector.app`. Drag it into `/Applications`.

The script signs the app with your first "Apple Development" certificate if
you have one (set `SIGN_IDENTITY` to choose another, or `SIGN_IDENTITY=-` for
an ad-hoc signature). macOS ties Full Disk Access to the signature: with a
certificate the grant survives rebuilds, with an ad-hoc signature it has to
be granted again after every build.

For development:

```bash
cargo run -- ~/Downloads
```

To run the tests:

```bash
cargo test
```

## Full Disk Access

Without Full Disk Access, macOS asks separately before the app can read
Desktop, Documents, Downloads, iCloud Drive, other apps' data, cloud storage
folders and external drives, and hides Mail, Messages and Safari data
completely. Turning on Full Disk Access once covers all of them, with no
prompts.

Open **Settings** in the app. It shows whether access is granted and has
buttons to open the Full Disk Access page in System Settings, show the app in
Finder (to drag it into the list), and relaunch, which macOS needs before the
change applies.

Until access is granted, the **Skip folders that would make macOS ask for
permission** setting (on by default) leaves those folders out of scans, so
you never get a stream of prompts. Skipped folders are listed under
"skipped or unreadable" in the status bar. Turn the setting off to let macOS
ask for each folder instead. A folder you pick to scan yourself is never
skipped.

## AI assistant

Space Hog Inspector can ask the AI agent you already have installed whether
an item is safe to remove. It works with **Claude Code** (`claude`) and
**Codex** (`codex`), runs them in the background, and uses your own sign-in
and plan: the app has no account or API key of its own.

Choose the agent in **Settings → AI Assistant** (it's off until you do), then
right-click any item and choose **Ask AI…**. The **AI** tab in the side panel
shows a verdict (safe, likely safe, caution or keep), what the item is, the
risks, whether it comes back on its own, and a safer way to remove it when
there is one. **Move to Trash…** there uses the usual confirmation, which
also shows the verdict. Answers are remembered until the app quits or the
item changes size. AI advice can be wrong, so check before removing anything.

What the agent is sent: the item's path (with your home folder shown as
`~`), its size, type and age, and the names and sizes of the 15 largest items
inside it. It is never sent file contents.

- **Claude Code** runs with every tool switched off and without your
  settings, hooks, plugins or MCP servers, so it can only answer from those
  facts.
- **Codex** can't have its tools switched off. It runs in its read-only
  sandbox with your Codex config ignored, but it can still look at files on
  disk while answering, using the app's Full Disk Access.

The app looks for the agents in `~/.local/bin`, `~/.claude/local`,
`/opt/homebrew/bin` and `/usr/local/bin`, then asks your login shell. If an
agent says it isn't signed in, run `claude` or `codex login` once in
Terminal.

## Notes on the numbers

- Sizes use decimal units (1 GB = 1,000,000,000 bytes), like Finder.
- APFS clones and snapshots share storage in ways a file walk can't see, so
  the scanned total can differ from the volume's used space shown in the
  status bar. Local Time Machine snapshots and purgeable space are the usual
  cause.
- Space from an item you move to the Trash isn't freed until you empty the
  Trash.

## Project layout

| Path | Purpose |
| --- | --- |
| `src/scanner.rs` | Parallel directory walk, size tree, file type classification |
| `src/treemap.rs` | Squarified treemap layout |
| `src/app.rs` | The egui interface: toolbar, treemap, side panel, dialogs, Settings window |
| `src/settings.rs` | Saved settings, Full Disk Access check, relaunch helpers |
| `src/ai.rs` | Finding and asking the installed AI agent (Claude Code or Codex) |
| `src/platform/` | Everything that differs by operating system: `macos.rs`, and `windows.rs` (a Windows port in progress, not yet tested on Windows) |
| `assets/icon-256.png` | The app icon for the Windows window and taskbar |
| `src/theme.rs` | Colours, fonts, egui styling, and the neon drawing helpers and controls |
| `assets/fonts/` | Chakra Petch, Inter and JetBrains Mono, each with its SIL Open Font License (`OFL-*.txt`) |
| `packaging/` | `bundle.sh`, `Info.plist`, and the icon generator |
