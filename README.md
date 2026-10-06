<p align="center">
  <img src="data/icons/scalable/apps/io.github.hematite.Editor.svg" width="128" height="128" alt="Hematite app icon">
</p>

# Hematite

A native Markdown editor for Linux, built with Rust, GTK 4, and libadwaita.
Notes are plain files in a vault folder. Hematite opens `~/Documents/Vault`
by default and creates it if it doesn't exist.

## Features

- **Notes and folders:** create, rename, and organize notes from the sidebar.
  Right-click a note or folder to rename it or move it to Trash.
  Drag a note onto a folder to move it; drop on **Vault** or empty sidebar space
  to move it back to the root. Relative image and file links are updated.
- **Search:** find notes by title or content, with matching text previews.
- **Markdown:** headings, bold, italics, bullets, clickable task checkboxes,
  links, and tables. Formatting markers appear when you edit a line.
- **Images:** paste images from the clipboard. They're saved in the vault and
  displayed inside the note, without cluttering the sidebar.
- **Autosave:** enable it in **Options → Preferences**, or save manually.
- **WebDAV sync:** keep a local copy of your vault and sync it with a server.

## Build and run

Requires Rust/Cargo, GTK **4.12+**, libadwaita **1.5+**, `pkg-config`, and
`glib-compile-resources`. Sync uses `curl`; saving passwords requires
`secret-tool`.

On Fedora, install the dependencies:

```sh
sudo dnf install rust cargo gtk4-devel libadwaita-devel pkgconf-pkg-config glib2-devel curl libsecret
```

Build and launch:

```sh
cargo build --locked
cargo run --locked
```

To open another vault:

```sh
cargo run --locked -- --vault /path/to/vault
```

A desktop launcher template is provided in
[data/io.github.hematite.Editor.desktop](data/io.github.hematite.Editor.desktop).

## Sync

Open **Options → Connect WebDAV server** and enter the server address,
username, and password. Use a full URL such as `https://example.org/vault/`,
or a domain name; Hematite tries the root and then `/vault/`.

- Saves go to disk first, so you can work offline. Pending changes sync when
  the connection returns.
- Remote files appear in the sidebar as they download.
- Hematite checks for changes every 30 seconds and when you return to its window.
- The bottom-right icon shows progress, success, or a problem. Click it for
  details or to sync now.
- Conflicting edits are kept as separate files. There is no automatic text merge.
- Hidden files and folders, including `.obsidian`, are excluded from sync.
- Passwords can be stored in the system keyring. Disconnecting keeps local files.

The server must provide strong ETags and support conditional writes or exclusive
WebDAV write locks. Hematite checks this when connecting; rclone's WebDAV server
works through the lock-based method.

## Keyboard shortcuts

| Shortcut | Action |
| --- | --- |
| Ctrl+N | New note |
| Ctrl+F | Search notes |
| Ctrl+S | Save |
| Ctrl+B / Ctrl+I | Bold / italic |
| Ctrl+Z / Ctrl+Shift+Z | Undo / redo |
| Ctrl+V | Paste text or an image |
| Ctrl+click / Ctrl+Enter | Open a link while editing its line |
