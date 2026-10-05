# Hematite

A minimal Rust / GTK 4 / libadwaita text editor with a vault sidebar and a
resizable Markdown editing pane. Defaults to `~/Documents/Vault`.

The app's original, fully vector hematite-stone icon is
[`data/icons/scalable/apps/io.github.hematite.Editor.svg`](data/icons/scalable/apps/io.github.hematite.Editor.svg).
It is embedded in the executable; building requires `glib-compile-resources`.
A desktop launcher template is in `data/io.github.hematite.Editor.desktop`.

Open the top-right options menu and choose **Connect WebDAV server** to enter
the server URL, username, and password. For example, use
`https://example.org/vault/`, or enter `example.org` and Hematite will
try the root followed by `/vault/`. Full URLs can point to any WebDAV collection.
The current local vault is the offline copy; remote files are fetched into it.
If both copies contain different versions, both are preserved as conflict files.

Saving writes locally first and starts background sync. Local changes survive
offline periods and app restarts. Hematite also checks every 30 seconds, when
the window becomes active, and when the network becomes available. Creation,
rename, Trash, and external changes to local files are included. Attachments
upload before notes, and hidden files/folders (including `.obsidian`) and
symbolic links are excluded. Incoming deletions go through local Trash.

The bottom-right indicator shows a spinner during sync, a green check after
success, or a warning for failures/conflicts. Click it for details and **Sync
now**. An open note with unsaved edits is never replaced by a remote download.
If both sides changed, the remote version is preserved as
`name.conflict-remote-HASH.md` in both vaults before the local version is
uploaded to the original name. There is no automatic text merge.

Connection setup verifies safe-write support using a temporary probe file.
Downloaded files appear in the sidebar as they arrive; the sync indicator stays
busy until the entire sync completes. Each accepted download's history is saved
immediately so progress survives a later network failure.
Servers must supply strong ETags and enforce either HTTP conditional writes
or exclusive WebDAV write locks. rclone's server uses the lock-based path.
Servers with neither protection are refused. No sync-token extension is used.
Credentials can be remembered with `secret-tool` in the system keyring, or
kept only for the current session. Non-secret connection settings and sync
history live under `$XDG_DATA_HOME/hematite/sync/` (normally
`~/.local/share/hematite/sync/`). Runtime dependencies are `curl` and, for
remembered passwords, `secret-tool`. Disconnect keeps local files.

Click the + button or press Ctrl+N to create a note in the vault root. The
dropdown beside + offers **New note** and **New folder**. Folders are named
inline in the sidebar; right-click a folder and choose **New note here** to
create a note inside it.
The new note row at the top of the sidebar has the cursor before `.md`; press Enter to
create the file and begin editing, or Escape to cancel. Existing files are never
overwritten.
Right-click a note and choose Rename to edit its filename inline. Enter commits
the rename; Escape cancels. Renaming preserves unsaved edits in the open note.

```sh
cargo run
# Or open a different vault:
cargo run -- --vault /path/to/vault
```

The sidebar shows collapsible folders containing `.md`, `.markdown`, `.txt`,
and `.text` files. Entries in each folder are ordered by modification date,
newest first, with alphabetical ordering for ties. Saving a note updates its
position automatically. Click a folder's row to expand or collapse it. Hidden
directories and symbolic links are skipped. Search matches titles, relative
paths, and note contents, including unsaved text in the open note. All search
words must match somewhere in a note; matching ignores capitalization. Search
shows a flat list ranked by relevance (title matches first), then modification
date, with paths and short content previews. Opening a result selects and
scrolls to its first content match. Clearing search returns to the folder tree
with your expansion choices intact. Contents are indexed in the background
when opening or refreshing the vault and after saves. Refresh picks up external
content changes; unreadable notes remain searchable by name.
Ctrl+F focuses search, Enter opens the top result, and Escape clears search.
Refresh rereads the file tree. Click a note to open it. Save with Ctrl+S. A dot beside the title indicates unsaved changes. Switching notes and closing the window prompt for unsaved
changes. If a note has changed on disk, saving is refused so you can preserve
your edits before reopening it. Open Options → Preferences to enable Autosave,
which saves one second after typing stops and uses the same WebDAV sync flow as
Ctrl+S. Autosave is off by default; your choice is remembered across launches.
Undo and redo use the standard Ctrl+Z and Ctrl+Shift+Z shortcuts.

Basic Markdown renders directly in the editor: `**bold**`, `*italic*`, dash
bullets (`- `), Markdown links (`[label](https://example.org)`), and bare web
URLs. Markers are concealed except on the logical line at the caret; selecting
multiple lines reveals their markers too. Dash bullets display as `•` without
changing the file. Inline and fenced code keep their literal text.

Ctrl+B and Ctrl+I wrap or unwrap selected text, or insert marker pairs at the
caret. Click rendered links to open them; on the line being edited, use
Ctrl+click or Ctrl+Enter. Web and mail links use the desktop's default app.
Formatting and concealment preserve the Markdown source, save behavior, and
undo history.

Markdown pipe tables render with wrapped cells, a bold header, grid lines, and
left/center/right alignment from the separator row. Click a rendered row to
reveal its Markdown for editing; other rows stay rendered. The source is
preserved when saving, and table rendering adapts to the editor width.

Right-click a note in the folder tree or search results and choose **Move to
Trash**. The confirmation identifies the note and warns if it has unsaved
edits. The saved file goes to your desktop Trash, where it can be restored;
unsaved edits are discarded only after a successful move. Folders cannot be
trashed from the app. Shift+F10 or the Menu key opens the menu for a focused note.
There is no permanent-delete fallback if Trash is unavailable.

Notes stay as text files. This first version supports the basic formatting
above and does not create or rename notes or create the vault directory.

## Build dependencies

Rust / Cargo, GTK 4.12+ development headers, libadwaita 1.5+ development headers,
and pkg-config. On Fedora: `gtk4-devel libadwaita-devel rust cargo`.
The Rust dependencies are pinned in Cargo.lock.

```sh
cargo build --locked
cargo test --locked
```

The opt-in UI smoke test requires a desktop session and a scratch vault:

```sh
scratch=$(mktemp -d)
mkdir -p "$scratch/folder"
printf 'Nested fixture\n' > "$scratch/folder/nested note.md"
printf 'smoke fixture\n' > "$scratch/smoke.md"
HEMATITE_SMOKE_TEST=1 cargo run -- --vault "$scratch"
rm -rf -- "$scratch"
```

It checks folder expansion and search, opens the fixture, edits the text buffer, checks
undo/redo, saves through the save action, verifies the result, and closes the
window. Never point this test at your real vault.

Paste clipboard images with Ctrl+V or the editor's Paste menu. Images are saved
as uniquely named `paste-YYYY-MM-DD-HHMMSS.png` files in the vault root, and a
standard Markdown image link is inserted relative to the current note. Images
stay out of the note sidebar. Undo removes the insertion, and trashing a note
keeps its attachments. Local image links on their own line render centered at
the smaller of 70% of the available text width and 33% of the current screen
width, preserving aspect ratio and resizing with the window. Editing the line
reveals its Markdown source. Missing images and remote
image URLs remain readable as source.

Add `HEMATITE_MARKDOWN_SMOKE_TEST=1` to the smoke-test invocation to also check
formatting, line concealment, link navigation with a fake URI opener, formatting
shortcuts, undo/redo, preservation of saved Markdown source, clipboard image
pasting into nested notes, and ordinary text paste.

To also test both Trash menus, cancellation, error handling, and discarding
unsaved edits, use an isolated Trash inside the scratch vault. Run this from
the project directory on the home filesystem (desktop Trash may not be
available for `/tmp`):

```sh
scratch=$(mktemp -d "$PWD/.hematite-trash-test.XXXXXX")
mkdir -p "$scratch/folder" "$scratch/.test-data"
printf 'Nested fixture\n' > "$scratch/folder/nested note.md"
printf 'smoke fixture\n' > "$scratch/smoke.md"
XDG_DATA_HOME="$scratch/.test-data" HEMATITE_SMOKE_TEST=1 \
  HEMATITE_TRASH_SMOKE_TEST=1 cargo run -- --vault "$scratch"
rm -rf -- "$scratch"
```
