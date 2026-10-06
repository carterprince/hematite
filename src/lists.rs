use gtk::prelude::*;

fn continuation(line: &str) -> Option<(usize, String)> {
    let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
    let rest = &line[indent..];
    let (marker_end, marker) = if rest.starts_with("- ") || rest.starts_with("* ") {
        (2, rest[..2].to_string())
    } else {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 || !rest[digits..].starts_with(". ") {
            return None;
        }
        let number = rest[..digits].parse::<u64>().ok()?.checked_add(1)?;
        (digits + 2, format!("{number}. "))
    };
    Some((indent + marker_end, format!("{}{marker}", &line[..indent])))
}

pub(super) fn enter(buffer: &gtk::TextBuffer) -> bool {
    if buffer.has_selection() {
        return false;
    }
    let mut cursor = buffer.iter_at_mark(&buffer.get_insert());
    let mut start = cursor;
    start.set_line_offset(0);
    let mut end = cursor;
    end.forward_to_line_end();
    let line = buffer.text(&start, &end, true);
    let Some((prefix_bytes, next)) = continuation(&line) else {
        return false;
    };
    let prefix_chars = line[..prefix_bytes].chars().count() as i32;
    if cursor.line_offset() < prefix_chars {
        return false;
    }
    buffer.begin_user_action();
    if line[prefix_bytes..].trim().is_empty() {
        buffer.delete(&mut start, &mut end);
    } else {
        buffer.insert(&mut cursor, &format!("\n{next}"));
    }
    buffer.end_user_action();
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn continues_numbers_and_preserves_bullet_and_indent() {
        assert_eq!(continuation("3. text"), Some((3, "4. ".into())));
        assert_eq!(continuation("99. text"), Some((4, "100. ".into())));
        assert_eq!(continuation("  * text"), Some((4, "  * ".into())));
        assert_eq!(continuation("\t- text"), Some((3, "\t- ".into())));
        assert_eq!(continuation("ordinary text"), None);
        assert_eq!(continuation("3.text"), None);
    }
}
