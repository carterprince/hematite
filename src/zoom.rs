use super::*;

pub(super) fn install(editor: &Editor) {
    let size = Rc::new(Cell::new(15));
    let css = gtk::CssProvider::new();
    gtk::style_context_add_provider_for_display(
        &editor.view.display(),
        &css,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
    );
    let adjust: Rc<dyn Fn(i32)> = Rc::new(move |step| {
        let next = if step == 0 { 15 } else { (size.get() + step).clamp(8, 48) };
        size.set(next);
        css.load_from_string(&format!(".editor {{ font-size: {next}px; }}"));
    });
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    keys.connect_key_pressed({
        let adjust = adjust.clone();
        move |_, key, _, modifiers| {
            if !modifiers.contains(gtk::gdk::ModifierType::CONTROL_MASK)
                || modifiers.intersects(gtk::gdk::ModifierType::ALT_MASK | gtk::gdk::ModifierType::SUPER_MASK)
            {
                return glib::Propagation::Proceed;
            }
            let step = match key {
                gtk::gdk::Key::plus | gtk::gdk::Key::equal | gtk::gdk::Key::KP_Add => 1,
                gtk::gdk::Key::minus | gtk::gdk::Key::KP_Subtract => -1,
                gtk::gdk::Key::_0 | gtk::gdk::Key::KP_0 => 0,
                _ => return glib::Propagation::Proceed,
            };
            adjust(step);
            glib::Propagation::Stop
        }
    });
    editor.window.add_controller(keys);
    let scroll = gtk::EventControllerScroll::new(
        gtk::EventControllerScrollFlags::VERTICAL | gtk::EventControllerScrollFlags::DISCRETE,
    );
    scroll.set_propagation_phase(gtk::PropagationPhase::Capture);
    scroll.connect_scroll(move |controller, _, dy| {
        if !controller.current_event_state().contains(gtk::gdk::ModifierType::CONTROL_MASK) {
            return glib::Propagation::Proceed;
        }
        if dy != 0.0 {
            adjust(if dy < 0.0 { 1 } else { -1 });
        }
        glib::Propagation::Stop
    });
    editor.view.add_controller(scroll);
}
