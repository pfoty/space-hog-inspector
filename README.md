# Space Hog Inspector

A native macOS app, written in Rust, that shows where your disk space goes.
It is similar to SpaceSniffer: a scan draws your folders as a treemap, a set
of nested boxes where each box's area is how much disk space it takes up.

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
  Open, Copy Path, and **Move to Trash…**, which always asks first. Nothing is
  ever deleted permanently: you can restore items from the Trash in Finder.
- **Status bar**: the scan total, how long it took, the volume's free and
  total space, and a list of items macOS wouldn't let it read.

Keyboard: `Cmd+O` choose a folder · `Cmd+R` rescan · `Backspace` /
`Cmd+↑` go up a level.

## Build

You need the Rust toolchain (`brew install rustup && rustup default stable`;
the Homebrew rustup is keg-only, so add `/opt/homebrew/opt/rustup/bin` to your
`PATH`) and the Xcode command-line tools.

```bash
packaging/bundle.sh
```

This builds an optimised binary and produces
`target/release/Space Hog Inspector.app`. Drag it into `/Applications`.

For development:

```bash
cargo run -- ~/Downloads
```

To run the tests:

```bash
cargo test
```

## Full Disk Access

macOS privacy protection hides some folders (Mail, Messages, Safari, Photos
libraries, other users' homes, Time Machine data) from apps until you allow
it. Without access those folders show up empty and are listed under
"items couldn't be read" in the status bar.

To see everything, open **System Settings → Privacy & Security → Full Disk
Access**, add `Space Hog Inspector.app` and switch it on, then quit and
reopen the app. The welcome screen and the unreadable-items list both have a
link to that settings page.

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
| `src/app.rs` | The egui interface: toolbar, treemap, side panel, dialogs |
| `packaging/` | `bundle.sh`, `Info.plist`, and the icon generator |
