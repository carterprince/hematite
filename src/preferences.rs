use super::*;
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Deserialize, Serialize)]
struct Settings {
    #[serde(default = "default_on")]
    autosave: bool,
    #[serde(default = "default_on")]
    folders_start_expanded: bool,
}

fn default_on() -> bool {
    true
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            autosave: true,
            folders_start_expanded: true,
        }
    }
}
fn load() -> Settings {
    std::fs::read(glib::user_config_dir().join("hematite/preferences.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}
pub(super) fn folders_start_expanded() -> bool {
    load().folders_start_expanded
}

struct Preferences {
    editor: Editor,
    enabled: Cell<bool>,
    timer: RefCell<Option<glib::SourceId>>,
    path: PathBuf,
}

impl Preferences {
    fn schedule(self: &Rc<Self>) {
        if let Some(timer) = self.timer.borrow_mut().take() {
            timer.remove();
        }
        if !self.enabled.get() {
            return;
        }
        let path = self.editor.document.borrow().path.clone();
        let preferences = self.clone();
        let timer = glib::timeout_add_local_once(Duration::from_secs(1), move || {
            preferences.timer.borrow_mut().take();
            let editor = &preferences.editor;
            if preferences.enabled.get()
                && path.is_some()
                && editor.document.borrow().path == path
                && editor.view.is_editable()
                && editor.buffer.is_modified()
            {
                editor.save();
            }
        });
        *self.timer.borrow_mut() = Some(timer);
    }

    fn show(self: &Rc<Self>) {
        let dialog = adw::PreferencesDialog::builder()
            .title("Preferences")
            .build();
        let page = adw::PreferencesPage::new();
        let group = adw::PreferencesGroup::builder().title("Editing").build();
        let autosave = adw::SwitchRow::builder()
            .title("Autosave")
            .subtitle("Save notes automatically after you stop typing")
            .active(self.enabled.get())
            .build();
        autosave.connect_active_notify({
            let preferences = self.clone();
            move |row| {
                let settings = Settings {
                    autosave: row.is_active(),
                    folders_start_expanded: folders_start_expanded(),
                };
                let result = (|| -> Result<(), Box<dyn std::error::Error>> {
                    std::fs::create_dir_all(preferences.path.parent().unwrap())?;
                    let temporary = preferences.path.with_extension("json.tmp");
                    std::fs::write(&temporary, serde_json::to_vec(&settings)?)?;
                    std::fs::rename(temporary, &preferences.path)?;
                    Ok(())
                })();
                if let Err(error) = result {
                    preferences
                        .editor
                        .toasts
                        .add_toast(adw::Toast::new(&format!(
                            "Could not save preferences: {error}"
                        )));
                }
                preferences.enabled.set(settings.autosave);
                preferences.schedule();
            }
        });
        group.add(&autosave);
        page.add(&group);
        let folders = adw::SwitchRow::builder()
            .title("Folders start expanded")
            .subtitle("Expand folders when opening the vault")
            .active(folders_start_expanded())
            .build();
        folders.connect_active_notify({
            let preferences = self.clone();
            move |row| {
                let mut settings = load();
                settings.folders_start_expanded = row.is_active();
                let result = (|| -> Result<(), Box<dyn std::error::Error>> {
                    std::fs::create_dir_all(preferences.path.parent().unwrap())?;
                    let temporary = preferences.path.with_extension("json.tmp");
                    std::fs::write(&temporary, serde_json::to_vec(&settings)?)?;
                    std::fs::rename(temporary, &preferences.path)?;
                    Ok(())
                })();
                if let Err(error) = result {
                    preferences
                        .editor
                        .toasts
                        .add_toast(adw::Toast::new(&format!(
                            "Could not save preferences: {error}"
                        )));
                }
            }
        });
        let browsing = adw::PreferencesGroup::builder().title("Sidebar").build();
        browsing.add(&folders);
        page.add(&browsing);
        dialog.add(&page);
        dialog.present(Some(&self.editor.window));
    }
}

pub(super) fn install(editor: &Editor) {
    let path = glib::user_config_dir().join("hematite/preferences.json");
    let settings = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Settings>(&bytes).ok())
        .unwrap_or_default();
    let preferences = Rc::new(Preferences {
        editor: editor.clone(),
        enabled: Cell::new(settings.autosave),
        timer: RefCell::new(None),
        path,
    });
    editor.buffer.connect_changed({
        let preferences = preferences.clone();
        move |_| preferences.schedule()
    });
    let action = gio::SimpleAction::new("preferences", None);
    action.connect_activate(move |_, _| preferences.show());
    editor.window.add_action(&action);
}

pub(super) async fn smoke(editor: &Editor) {
    fn switch(widget: &gtk::Widget) -> Option<adw::SwitchRow> {
        if let Ok(row) = widget.clone().downcast::<adw::SwitchRow>() {
            return Some(row);
        }
        let mut child = widget.first_child();
        while let Some(widget) = child {
            if let Some(row) = switch(&widget) {
                return Some(row);
            }
            child = widget.next_sibling();
        }
        None
    }
    gtk::prelude::WidgetExt::activate_action(&editor.window, "win.preferences", None).unwrap();
    let dialog = editor.window.visible_dialog().unwrap();
    assert!(dialog.is::<adw::PreferencesDialog>());
    let row = switch(dialog.upcast_ref()).unwrap();
    assert!(row.is_active());
    row.set_active(false);
    row.set_active(true);
    editor.buffer.insert_at_cursor("\nautosave fixture");
    glib::timeout_future(Duration::from_millis(1300)).await;
    let path = editor.document.borrow().path.clone().unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), editor.text());
    assert!(!editor.buffer.is_modified());
    row.set_active(false);
    let settings: Settings = serde_json::from_slice(
        &std::fs::read(glib::user_config_dir().join("hematite/preferences.json")).unwrap(),
    )
    .unwrap();
    assert!(!settings.autosave);
    let saved = editor.text();
    editor.buffer.insert_at_cursor("\nmanual save fixture");
    glib::timeout_future(Duration::from_millis(1300)).await;
    assert!(editor.buffer.is_modified());
    assert_eq!(std::fs::read_to_string(path).unwrap(), saved);
    assert!(editor.save());
    dialog.close();
    println!("Preferences verified: autosave enabled saves; disabled retains unsaved changes.");
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn folders_expand_by_default_including_older_settings() {
        assert!(Settings::default().folders_start_expanded);
        assert!(Settings::default().autosave);
        let old: Settings = serde_json::from_str(r#"{"autosave":true}"#).unwrap();
        assert!(old.autosave && old.folders_start_expanded);
        let settings: Settings =
            serde_json::from_str(r#"{"autosave":false,"folders_start_expanded":false}"#).unwrap();
        assert!(!settings.folders_start_expanded);
        assert!(!settings.autosave);
    }
}
