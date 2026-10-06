use super::*;
use crate::{
    sync_dav::{Dav, hash, normalize_url},
    sync_engine::{self, State},
};
use serde::{Deserialize, Serialize};
use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

#[derive(Clone, Serialize, Deserialize)]
struct Profile {
    url: String,
    username: String,
    lock_writes: bool,
    #[serde(default)]
    remember: bool,
}
#[derive(Clone)]
struct Session {
    dav: Dav,
    state: State,
}
pub struct Controller {
    editor: Editor,
    button: gtk::Button,
    icon: gtk::Image,
    spinner: gtk::Spinner,
    session: RefCell<Option<Session>>,
    running: Cell<bool>,
    again: Cell<bool>,
    generation: Cell<u64>,
    detail: RefCell<String>,
    directory: PathBuf,
    fresh_checkout: Cell<bool>,
}
fn data_directory(root: &Path) -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap()).join(".local/share"));
    base.join("hematite/sync")
        .join(hash(root.to_string_lossy().as_bytes()))
}
fn write_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let parent = path.parent().unwrap();
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
    }
    let temporary = parent.join(format!(".write-{}", glib::uuid_string_random()));
    std::fs::write(
        &temporary,
        serde_json::to_vec(value).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    std::fs::rename(temporary, path).map_err(|e| e.to_string())
}
fn state_from(directory: &Path) -> Result<State, String> {
    match std::fs::read(directory.join("state.json")) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| {
            "Sync history is damaged. Disconnect and reconnect to reconcile safely.".into()
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
        Err(error) => Err(error.to_string()),
    }
}
fn keyring(profile: &Profile, root: &Path, password: Option<&str>) -> Result<String, String> {
    let key = hash(format!("{}\n{}\n{}", root.display(), profile.url, profile.username).as_bytes());
    let mut command = Command::new("secret-tool");
    if password.is_some() {
        command.args(["store", "--label=Hematite WebDAV vault"]);
    } else {
        command.arg("lookup");
    }
    command.args(["application", "hematite", "vault", &key]);
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| "secret-tool is required to remember passwords in the system keyring")?;
    if let Some(password) = password {
        child
            .stdin
            .take()
            .unwrap()
            .write_all(password.as_bytes())
            .map_err(|e| e.to_string())?;
    } else {
        drop(child.stdin.take());
    }
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err("Could not access the system keyring. Unlock it, or connect without remembering your password.".into());
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .trim_end_matches('\n')
        .to_string())
}

impl Controller {
    fn receive_download(&self, change: &sync_engine::Change) -> bool {
        let path = self.editor.root.join(&change.path);
        if self.session.borrow().as_ref().is_some_and(|session| {
            session.state.deletions.iter().any(|deleted| {
                change.path == *deleted || change.path.starts_with(&format!("{deleted}/"))
            })
        }) {
            return false;
        }
        let current = self.editor.document.borrow().path.as_ref() == Some(&path);
        if current && self.editor.buffer.is_modified() {
            return false;
        }
        if !matches!(sync_engine::apply(&self.editor.root, change), Ok(true)) {
            return false;
        }
        // Persist each accepted download so a later network failure doesn't lose
        // its baseline and make it look like a new local upload on the retry.
        if let Some(session) = self.session.borrow_mut().as_mut() {
            if let Some(record) = &change.record {
                session
                    .state
                    .files
                    .insert(change.path.clone(), record.clone());
            } else {
                session.state.files.remove(&change.path);
            }
            if let Err(error) = write_json(&self.directory.join("state.json"), &session.state) {
                self.editor.toasts.add_toast(adw::Toast::new(&error));
            }
        }
        self.editor
            .view
            .clone()
            .downcast::<markdown::View>()
            .unwrap()
            .refresh_images();
        if current {
            if path.exists() {
                let caret = self.editor.buffer.cursor_position();
                let focused = self.editor.view.has_focus();
                self.editor.open(&path);
                self.editor.buffer.place_cursor(
                    &self
                        .editor
                        .buffer
                        .iter_at_offset(caret.min(self.editor.buffer.char_count())),
                );
                if !focused {
                    self.button.grab_focus();
                }
            } else {
                *self.editor.document.borrow_mut() = Document::default();
                self.editor
                    .buffer
                    .set_text("This note was deleted on another device.");
                self.editor.buffer.set_modified(false);
                self.editor.update();
            }
        }
        self.editor.refresh_button.emit_clicked();
        true
    }
    fn busy(&self, message: &str) {
        self.icon.set_visible(false);
        self.spinner.set_visible(true);
        self.spinner.start();
        self.button.set_tooltip_text(Some(message));
        self.button
            .update_property(&[gtk::accessible::Property::Label(message)]);
        *self.detail.borrow_mut() = message.into();
    }
    fn status(&self, icon: &str, detail: &str, error: bool) {
        self.spinner.stop();
        self.spinner.set_visible(false);
        self.icon.set_visible(true);
        self.icon.set_icon_name(Some(icon));
        self.icon.remove_css_class("success");
        self.icon.remove_css_class("error");
        if error || self.session.borrow().is_some() {
            self.icon
                .add_css_class(if error { "error" } else { "success" });
        }
        *self.detail.borrow_mut() = detail.to_string();
        self.button.set_tooltip_text(Some(detail));
        self.button
            .update_property(&[gtk::accessible::Property::Label(detail)]);
    }
    pub fn request(self: &Rc<Self>) {
        if self.session.borrow().is_none() {
            return;
        }
        if self.running.replace(true) {
            self.again.set(true);
            return;
        }
        self.busy("Syncing vault…");
        let session = self.session.borrow().as_ref().unwrap().clone();
        let initial_deletions = session.state.deletions.clone();
        let sync_external = crate::preferences::sync_external_deletions();
        let fresh_checkout = self.fresh_checkout.get() || !self.editor.root.is_dir();
        let root = self.editor.root.clone();
        let generation = self.generation.get();
        let controller = self.clone();
        let (downloads, incoming) =
            std::sync::mpsc::channel::<(sync_engine::Change, std::sync::mpsc::Sender<bool>)>();
        let progress_controller = self.clone();
        let progress = glib::timeout_add_local(std::time::Duration::from_millis(25), move || {
            loop {
                match incoming.try_recv() {
                    Ok((change, accepted)) => {
                        let applied = generation == progress_controller.generation.get()
                            && progress_controller.receive_download(&change);
                        let _ = accepted.send(applied);
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
                }
            }
            glib::ControlFlow::Continue
        });
        glib::spawn_future_local(async move {
            let work = gio::spawn_blocking(move || {
                let missing_vault = fresh_checkout || !root.is_dir();
                let local = sync_engine::snapshot(&root)?;
                sync_engine::cycle_with_policy(
                    &session.dav,
                    local,
                    session.state,
                    sync_external,
                    missing_vault,
                    |change| {
                        let (accepted, result) = std::sync::mpsc::channel();
                        if downloads.send((change.clone(), accepted)).is_err() {
                            return false;
                        }
                        result.recv().unwrap_or(false)
                    },
                )
            })
            .await;
            progress.remove();
            controller.running.set(false);
            if generation != controller.generation.get() {
                if controller.again.replace(false) {
                    controller.request();
                }
                return;
            }
            match work {
                Ok(Ok(mut outcome)) => {
                    let mut waiting = false; let mut changed = false; let mut current_changed = false; let mut errors = Vec::new();
                    controller.fresh_checkout.set(false);
                    let pending_deletions: std::collections::BTreeSet<_> = controller.session.borrow().as_ref().map(|session| session.state.deletions.difference(&initial_deletions).cloned().collect()).unwrap_or_default();
                    outcome.state.deletions.extend(pending_deletions.iter().cloned());
                    let current = controller.editor.document.borrow().path.clone();
                    for change in outcome.changes {
                        let path = controller.editor.root.join(&change.path);
                        if pending_deletions.iter().any(|deleted| change.path == *deleted || change.path.starts_with(&format!("{deleted}/"))) { waiting = true; continue; }
                        if current.as_ref() == Some(&path) && controller.editor.buffer.is_modified() {
                            waiting = true; continue;
                        }
                        match sync_engine::apply(&controller.editor.root, &change) {
                            Ok(true) => {
                                changed = true;
                                current_changed |= current.as_ref() == Some(&path);
                                if let Some(record) = change.record { outcome.state.files.insert(change.path, record); }
                                else { outcome.state.files.remove(&change.path); }
                            }
                            Ok(false) => waiting = true,
                            Err(error) => errors.push(error),
                        }
                    }
                    if let Some(session) = controller.session.borrow_mut().as_mut() { session.state = outcome.state.clone(); }
                    if let Err(error) = write_json(&controller.directory.join("state.json"), &outcome.state) { errors.push(error); }
                    if changed {
                        controller.editor.view.clone().downcast::<markdown::View>().unwrap().refresh_images();
                        if let Some(path) = current.filter(|_| current_changed) {
                            if !controller.editor.buffer.is_modified() {
                                if path.exists() {
                                    let caret = controller.editor.buffer.cursor_position();
                                    let focused = controller.editor.view.has_focus();
                                    controller.editor.open(&path);
                                    controller.editor.buffer.place_cursor(&controller.editor.buffer.iter_at_offset(caret.min(controller.editor.buffer.char_count())));
                                    if !focused { controller.button.grab_focus(); }
                                } else {
                                    *controller.editor.document.borrow_mut() = Document::default();
                                    controller.editor.buffer.set_text("This note was deleted on another device.");
                                    controller.editor.buffer.set_modified(false);
                                    controller.editor.update();
                                }
                            }
                        }
                        controller.editor.refresh_button.emit_clicked();
                    }
                    if !errors.is_empty() { controller.status("dialog-warning-symbolic", &errors.join("\n"), true); }
                    else if !outcome.conflicts.is_empty() {
                        let message = format!("Sync completed with conflicts. Both versions were preserved:\n{}", outcome.conflicts.join("\n"));
                        controller.status("dialog-warning-symbolic", &message, true);
                        controller.editor.toasts.add_toast(adw::Toast::new("Sync conflict: both versions have been preserved."));
                    } else if waiting { controller.status("appointment-soon-symbolic", "Remote changes are waiting. Save or discard your edits to continue syncing.", true); }
                    else { controller.status("object-select-symbolic", "Vault synced", false); }
                }
                Ok(Err(error)) => controller.status("dialog-warning-symbolic", &format!("Sync failed. Local saves are safe and pending changes will retry.\n\n{error}"), true),
                Err(_) => controller.status("dialog-warning-symbolic", "Sync worker failed. Your local files are safe; changes will retry.", true),
            }
            if controller.again.replace(false) {
                controller.request();
            }
        });
    }
    fn disconnect(&self) {
        self.generation.set(self.generation.get() + 1);
        self.session.borrow_mut().take();
        self.again.set(false);
        if let Err(error) = std::fs::remove_file(self.directory.join("profile.json")) {
            if error.kind() != std::io::ErrorKind::NotFound {
                self.status("dialog-warning-symbolic", &format!("Disconnected for this session, but could not remove saved connection settings: {error}"), true);
                return;
            }
        }
        self.status(
            "network-offline-symbolic",
            "WebDAV disconnected. Local files have been kept.",
            false,
        );
    }
    fn connect_dialog(self: &Rc<Self>) {
        let dialog = adw::AlertDialog::builder().heading("Connect WebDAV server")
            .body("Sync this local vault with a WebDAV server. Local saves work offline. Different versions are preserved as conflict copies.").build();
        let form = gtk::Box::new(gtk::Orientation::Vertical, 10);
        let url = gtk::Entry::new();
        url.set_placeholder_text(Some("https://example.org/vault/"));
        let username = gtk::Entry::new();
        username.set_placeholder_text(Some("Username"));
        let password = gtk::PasswordEntry::new();
        password.set_show_peek_icon(true);
        password.set_placeholder_text(Some("Password"));
        if let Ok(bytes) = std::fs::read(self.directory.join("profile.json")) {
            if let Ok(profile) = serde_json::from_slice::<Profile>(&bytes) {
                url.set_text(&profile.url);
                username.set_text(&profile.username);
            }
        }
        let remember = gtk::CheckButton::with_label("Remember password in the system keyring");
        remember.set_active(true);
        for (label, widget) in [
            ("Server URL", url.clone().upcast::<gtk::Widget>()),
            ("Username", username.clone().upcast()),
            ("Password", password.clone().upcast()),
        ] {
            let label = gtk::Label::new(Some(label));
            label.set_xalign(0.0);
            form.append(&label);
            form.append(&widget);
        }
        form.append(&remember);
        dialog.set_extra_child(Some(&form));
        dialog.add_responses(&[("cancel", "Cancel"), ("connect", "Connect")]);
        dialog.set_response_appearance("connect", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("connect"));
        dialog.set_close_response("cancel");
        dialog.connect_response(None, {
            let controller = self.clone();
            move |_, response| {
                if response != "connect" { return; }
                let typed_url = url.text().to_string();
                let normalized = match normalize_url(&typed_url) { Ok(url) => url, Err(error) => { controller.editor.error(&error); return; } };
                if username.text().contains([':', '\n', '\r']) || password.text().contains(['\n', '\r']) {
                    controller.editor.error("Username and password cannot contain newlines; username cannot contain a colon."); return;
                }
                let bare_domain = !typed_url.contains('/') && !typed_url.contains("://");
                let mut dav = Dav { url: normalized, username: username.text().to_string(), password: password.text().to_string(), lock_writes: false };
                password.set_text("");
                let remember = remember.is_active();
                controller.generation.set(controller.generation.get()+1);
                let generation = controller.generation.get();
                controller.session.borrow_mut().take();
                controller.busy("Connecting and checking safe-write support…");
                let directory = controller.directory.clone(); let root = controller.editor.root.clone();
                let controller = controller.clone();
                glib::spawn_future_local(async move {
                    let connection = gio::spawn_blocking(move || {
                        if dav.list().is_err() && bare_domain { dav.url.push_str("vault/"); }
                        dav.list()?; dav.probe()?;
                        let profile = Profile { url: dav.url.clone(), username: dav.username.clone(), lock_writes: dav.lock_writes, remember };
                        let previous = std::fs::read(directory.join("profile.json")).ok().and_then(|bytes| serde_json::from_slice::<Profile>(&bytes).ok());
                        let state = if previous.as_ref().is_some_and(|old| old.url == profile.url && old.username == profile.username) { state_from(&directory)? } else { State::default() };
                        if remember { keyring(&profile, &root, Some(&dav.password))?; }
                        Ok::<_, String>((Session { dav, state }, profile))
                    }).await;
                    if generation != controller.generation.get() { return; }
                    match connection {
                        Ok(Ok((session, profile))) => {
                            let saved = write_json(&controller.directory.join("profile.json"), &profile)
                                .and_then(|_| write_json(&controller.directory.join("state.json"), &session.state));
                            if let Err(error) = saved { controller.status("dialog-warning-symbolic", &error, true); return; }
                            *controller.session.borrow_mut() = Some(session); controller.request();
                        }
                        Ok(Err(error)) => { controller.status("dialog-warning-symbolic", &error, true); controller.editor.error(&error); }
                        Err(_) => controller.status("dialog-warning-symbolic", "Connection worker failed", true),
                    }
                });
            }
        });
        dialog.present(Some(&self.editor.window));
    }
}

// Record only operations performed inside Hematite; persists through offline
// periods/restarts and also covers the old paths of renames and moves.
pub fn vault_missing(editor: &Editor) {
    if let Some(controller) = editor.sync.borrow().as_ref() {
        controller.fresh_checkout.set(true);
        controller.generation.set(controller.generation.get() + 1);
        if controller.running.get() {
            controller.again.set(true);
        }
        // Reset on disk before the sidebar recreates the directory. Otherwise
        // a restart after an interrupted download could reuse deletion history.
        let state = State::default();
        if let Some(session) = controller.session.borrow_mut().as_mut() {
            session.state = state.clone();
        }
        if controller.directory.exists() {
            if let Err(error) = write_json(&controller.directory.join("state.json"), &state) {
                editor.toasts.add_toast(adw::Toast::new(&format!(
                    "Could not reset missing-vault sync history: {error}"
                )));
            }
        }
    }
}

pub fn record_deletion(editor: &Editor, path: &Path) {
    let Some(controller) = editor.sync.borrow().clone() else {
        return;
    };
    let relative = match path.strip_prefix(&editor.root) {
        Ok(path) => path
            .to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/"),
        Err(_) => return,
    };
    let mut state = controller
        .session
        .borrow()
        .as_ref()
        .map(|session| session.state.clone())
        .unwrap_or_else(|| state_from(&controller.directory).unwrap_or_default());
    let paths: Vec<_> = state
        .files
        .keys()
        .filter(|name| *name == &relative || name.starts_with(&format!("{relative}/")))
        .cloned()
        .collect();
    state.deletions.extend(paths);
    state.deletions.insert(relative);
    if let Err(error) = write_json(&controller.directory.join("state.json"), &state) {
        editor.toasts.add_toast(adw::Toast::new(&format!(
            "Could not remember deletion for sync: {error}"
        )));
    }
    if let Some(session) = controller.session.borrow_mut().as_mut() {
        session.state = state;
    }
}

pub fn request(editor: &Editor) {
    if let Some(controller) = editor.sync.borrow().as_ref() {
        controller.request();
    }
}
pub fn install(editor: &Editor, menu_button: &gtk::MenuButton, status_button: &gtk::Button) {
    let icon = gtk::Image::from_icon_name("network-offline-symbolic");
    let spinner = gtk::Spinner::new();
    spinner.set_visible(false);
    let icons = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    icons.append(&icon);
    icons.append(&spinner);
    status_button.set_child(Some(&icons));
    status_button.add_css_class("flat");
    let controller = Rc::new(Controller {
        editor: editor.clone(),
        button: status_button.clone(),
        icon,
        spinner,
        session: RefCell::default(),
        running: Cell::new(false),
        again: Cell::new(false),
        generation: Cell::new(0),
        detail: RefCell::default(),
        directory: data_directory(&editor.root),
        fresh_checkout: Cell::new(!editor.root.is_dir()),
    });
    controller.status("network-offline-symbolic", "WebDAV is not connected", false);
    *editor.sync.borrow_mut() = Some(controller.clone());
    if !editor.root.is_dir() {
        vault_missing(editor);
    }
    status_button.connect_clicked({
        let controller = controller.clone();
        move |_| {
            let dialog = adw::AlertDialog::builder()
                .heading("Vault sync")
                .body(controller.detail.borrow().as_str())
                .build();
            dialog.add_responses(&[("close", "Close"), ("retry", "Sync now")]);
            dialog.connect_response(None, {
                let controller = controller.clone();
                move |_, response| {
                    if response == "retry" {
                        controller.request();
                    }
                }
            });
            dialog.present(Some(&controller.editor.window));
        }
    });
    let model = gio::Menu::new();
    model.append(Some("Preferences"), Some("win.preferences"));
    model.append(Some("Connect WebDAV server"), Some("win.connect-webdav"));
    model.append(Some("Sync now"), Some("win.sync-now"));
    model.append(
        Some("Disconnect WebDAV server"),
        Some("win.disconnect-webdav"),
    );
    menu_button.set_menu_model(Some(&model));
    for name in ["connect-webdav", "sync-now", "disconnect-webdav"] {
        let action = gio::SimpleAction::new(name, None);
        let controller = controller.clone();
        action.connect_activate(move |_, _| match name {
            "connect-webdav" => controller.connect_dialog(),
            "sync-now" => controller.request(),
            _ => controller.disconnect(),
        });
        editor.window.add_action(&action);
    }
    let weak = Rc::downgrade(&controller);
    glib::timeout_add_seconds_local(30, move || {
        let Some(controller) = weak.upgrade() else {
            return glib::ControlFlow::Break;
        };
        controller.request();
        glib::ControlFlow::Continue
    });
    editor.window.connect_is_active_notify({
        let controller = controller.clone();
        move |window| {
            if window.is_active() {
                controller.request();
            }
        }
    });
    gio::NetworkMonitor::default().connect_network_changed({
        let controller = controller.clone();
        move |_, available| {
            if available {
                controller.request();
            }
        }
    });
    if std::env::var_os("HEMATITE_SMOKE_TEST").is_none() {
        if let Ok(bytes) = std::fs::read(controller.directory.join("profile.json")) {
            if let Ok(profile) = serde_json::from_slice::<Profile>(&bytes) {
                if !profile.remember {
                    controller.status(
                        "network-offline-symbolic",
                        "Reconnect WebDAV to enter your password. Local changes have been kept.",
                        false,
                    );
                    return;
                }
                let generation = controller.generation.get();
                let directory = controller.directory.clone();
                let root = editor.root.clone();
                glib::spawn_future_local(async move {
                    let restored = gio::spawn_blocking(move || {
                        let password = keyring(&profile, &root, None)?;
                        if password.is_empty() {
                            return Err(
                                "Reconnect your WebDAV server to unlock or enter its password."
                                    .into(),
                            );
                        }
                        let mut dav = Dav {
                            url: profile.url,
                            username: profile.username,
                            password,
                            lock_writes: profile.lock_writes,
                        };
                        dav.list()?;
                        dav.probe()?;
                        Ok::<_, String>(Session {
                            dav,
                            state: state_from(&directory)?,
                        })
                    })
                    .await;
                    if generation != controller.generation.get() {
                        return;
                    }
                    match restored {
                        Ok(Ok(session)) => {
                            *controller.session.borrow_mut() = Some(session);
                            controller.request();
                        }
                        Ok(Err(error)) => {
                            controller.status("dialog-warning-symbolic", &error, true)
                        }
                        Err(_) => controller.status(
                            "dialog-warning-symbolic",
                            "Could not restore WebDAV connection",
                            true,
                        ),
                    }
                });
            }
        }
    }
}

pub(super) async fn smoke(editor: &Editor) {
    async fn settle(controller: &Controller) {
        for _ in 0..600 {
            glib::timeout_future(std::time::Duration::from_millis(100)).await;
            if controller.session.borrow().is_some() && !controller.running.get() {
                assert!(
                    !controller.detail.borrow().starts_with("Sync failed"),
                    "{}",
                    controller.detail.borrow()
                );
                return;
            }
        }
        panic!("Sync timed out: {}", controller.detail.borrow());
    }
    gtk::prelude::WidgetExt::activate_action(&editor.window, "win.connect-webdav", None).unwrap();
    let dialog = editor
        .window
        .visible_dialog()
        .unwrap()
        .downcast::<adw::AlertDialog>()
        .unwrap();
    let form = dialog.extra_child().unwrap();
    let mut fields = Vec::new();
    let mut child = form.first_child();
    while let Some(widget) = child {
        child = widget.next_sibling();
        fields.push(widget);
    }
    fields[1]
        .clone()
        .downcast::<gtk::Entry>()
        .unwrap()
        .set_text(&std::env::var("HEMATITE_TEST_WEBDAV_URL").unwrap());
    fields[3]
        .clone()
        .downcast::<gtk::Entry>()
        .unwrap()
        .set_text(&std::env::var("HEMATITE_TEST_WEBDAV_USER").unwrap());
    fields[5]
        .clone()
        .downcast::<gtk::PasswordEntry>()
        .unwrap()
        .set_text(&std::env::var("HEMATITE_TEST_WEBDAV_PASSWORD").unwrap());
    fields[6]
        .clone()
        .downcast::<gtk::CheckButton>()
        .unwrap()
        .set_active(false);
    dialog.emit_by_name::<()>("response", &[&"connect"]);
    dialog.close();
    if std::env::var_os("HEMATITE_PROGRESS_SMOKE_TEST").is_some() {
        let controller = editor.sync.borrow().as_ref().unwrap().clone();
        let mut appeared_during_sync = false;
        for _ in 0..1200 {
            glib::timeout_future(std::time::Duration::from_millis(25)).await;
            if editor.root.join("000-progress.md").exists() && controller.running.get() {
                let mut row = editor.list.first_child();
                while let Some(widget) = row {
                    if widget.tooltip_text().as_deref() == Some("000-progress.md") {
                        appeared_during_sync = true;
                        break;
                    }
                    row = widget.next_sibling();
                }
                if appeared_during_sync {
                    break;
                }
            }
        }
        assert!(
            appeared_during_sync,
            "Downloaded note must appear in the sidebar before sync completes"
        );
        println!("Progress verified: downloaded note appeared while sync was still running.");
    }
    let controller = editor.sync.borrow().as_ref().unwrap().clone();
    settle(&controller).await;
    assert!(
        controller
            .session
            .borrow()
            .as_ref()
            .unwrap()
            .dav
            .lock_writes
    );
    assert_eq!(
        controller.icon.icon_name().as_deref(),
        Some("object-select-symbolic")
    );
    let path = editor.root.join("remote fixture.md");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "Remote fixture\n");
    editor.open(&path);
    editor.buffer.insert_at_cursor("Local save\n");
    assert!(editor.save());
    assert!(controller.spinner.is_visible());
    settle(&controller).await;
    let local = editor.text();
    let dav = controller.session.borrow().as_ref().unwrap().dav.clone();
    let uploaded = gio::spawn_blocking({
        let dav = dav.clone();
        move || dav.get("remote fixture.md").unwrap().0
    })
    .await
    .unwrap();
    assert_eq!(uploaded, local.as_bytes());
    controller.session.borrow_mut().as_mut().unwrap().dav.url = "http://127.0.0.1:1/".into();
    editor.buffer.insert_at_cursor("Offline save\n");
    assert!(editor.save());
    for _ in 0..100 {
        glib::timeout_future(std::time::Duration::from_millis(100)).await;
        if !controller.running.get() {
            break;
        }
    }
    assert!(controller.detail.borrow().starts_with("Sync failed"));
    assert_eq!(
        controller.icon.icon_name().as_deref(),
        Some("dialog-warning-symbolic")
    );
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("Offline save")
    );
    controller.session.borrow_mut().as_mut().unwrap().dav.url = dav.url.clone();
    controller.request();
    settle(&controller).await;
    assert_eq!(
        controller.icon.icon_name().as_deref(),
        Some("object-select-symbolic")
    );
    editor.buffer.insert_at_cursor("Unsaved editor text\n");
    let unsaved = editor.text();
    gio::spawn_blocking({
        let dav = dav.clone();
        move || {
            let etag = dav.get("remote fixture.md").unwrap().1;
            dav.put(
                "remote fixture.md",
                b"Remote edit while typing\n",
                Some(&etag),
            )
            .unwrap();
        }
    })
    .await
    .unwrap();
    controller.request();
    settle(&controller).await;
    assert_eq!(editor.text(), unsaved);
    assert!(editor.buffer.is_modified());
    assert!(controller.detail.borrow().contains("waiting"));
    assert!(editor.save());
    settle(&controller).await;
    assert_eq!(editor.text(), unsaved);
    assert!(!editor.buffer.is_modified());
    assert!(
        sync_engine::snapshot(&editor.root)
            .unwrap()
            .keys()
            .any(|name| name.contains("conflict-remote"))
    );
    let profile = std::fs::read_to_string(controller.directory.join("profile.json")).unwrap();
    assert!(!profile.contains(&std::env::var("HEMATITE_TEST_WEBDAV_PASSWORD").unwrap()));
    assert!(!profile.contains("password"));
    if let Some(path) = std::env::var_os("HEMATITE_SYNC_SCREENSHOT") {
        let snapshot = gtk::Snapshot::new();
        gtk::WidgetPaintable::new(Some(&editor.window)).snapshot(
            &snapshot,
            editor.window.width().into(),
            editor.window.height().into(),
        );
        editor
            .window
            .renderer()
            .unwrap()
            .render_texture(&snapshot.to_node().unwrap(), None)
            .save_to_png(PathBuf::from(path))
            .unwrap();
    }
    controller.disconnect();
    assert!(path.exists());
    println!(
        "Desktop sync verified: setup dialog, bootstrap, save/upload, unsaved edit protection, conflict preservation, indicator, disconnect, and no plaintext password storage."
    );
}
