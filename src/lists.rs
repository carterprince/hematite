use gtk::prelude::*;

fn continuation(line: &str) -> Option<(usize, String)> {
    let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
    let rest = &line[indent..];
    let (marker_end, marker) = if rest.starts_with("- ") || rest.starts_with("* ") {
        let task = rest.as_bytes();
        if task.len() >= 6
            && task[2] == b'['
            && matches!(task[3], b' ' | b'x' | b'X')
            && task[4] == b']'
            && task[5] == b' '
        {
            (6, format!("{}[ ] ", &rest[..2]))
        } else {
            (2, rest[..2].to_string())
        }
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

fn tab_prefix(line: &str, preceding: &str, outdent: bool) -> Option<(usize, String)> {
    let (prefix, _) = continuation(line)?;
    let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
    if outdent {
        if indent == 0 {
            return None;
        }
        for previous in preceding.lines().rev() {
            let previous_indent = previous.len() - previous.trim_start_matches([' ', '\t']).len();
            if previous.trim().is_empty() || previous_indent >= indent {
                continue;
            }
            if let Some((_, next)) = continuation(previous) {
                return Some((prefix, next));
            }
            break;
        }
        let removed = if line.starts_with('\t') { 1 } else { indent.min(2) };
        Some((prefix, line[removed..prefix].to_string()))
    } else {
        let mut width = 2;
        for previous in preceding.lines().rev().filter(|line| !line.trim().is_empty()) {
            let previous_indent = previous.len() - previous.trim_start_matches([' ', '\t']).len();
            if previous_indent > indent {
                continue;
            }
            if previous_indent == indent {
                if let Some((previous_prefix, _)) = continuation(previous) {
                    width = previous_prefix - previous_indent;
                }
            }
            break;
        }
        let marker = if line[indent..].starts_with('*') { "* " } else { "- " };
        Some((prefix, format!("{}{}{marker}", &line[..indent], " ".repeat(width))))
    }
}

pub(super) fn tab(buffer: &gtk::TextBuffer, outdent: bool) -> bool {
    if buffer.has_selection() {
        return false;
    }
    let cursor = buffer.iter_at_mark(&buffer.get_insert());
    let mut start = cursor;
    start.set_line_offset(0);
    let mut end = cursor;
    end.forward_to_line_end();
    let line = buffer.text(&start, &end, true);
    let preceding = buffer.text(&buffer.start_iter(), &start, true);
    let Some((prefix, replacement)) = tab_prefix(&line, &preceding, outdent) else {
        return false;
    };
    let prefix_chars = line[..prefix].chars().count() as i32;
    if cursor.line_offset() < prefix_chars {
        return false;
    }
    let offset = cursor.offset() - prefix_chars + replacement.chars().count() as i32;
    end = start;
    end.forward_chars(prefix_chars);
    buffer.begin_user_action();
    buffer.delete(&mut start, &mut end);
    buffer.insert(&mut start, &replacement);
    buffer.place_cursor(&buffer.iter_at_offset(offset));
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

    #[test]
    fn tab_nests_under_number_and_shift_tab_restores_next_number() {
        assert_eq!(tab_prefix("4. ", "3. parent\n", false), Some((3, "   - ".into())));
        assert_eq!(tab_prefix("   - child", "3. parent\n   - sibling\n", true), Some((5, "4. ".into())));
        assert_eq!(tab_prefix("- child", "- parent\n", false), Some((2, "  - ".into())));
        assert_eq!(tab_prefix("  - child", "- parent\n", true), Some((4, "- ".into())));
    }

    #[test]
    fn checkbox_continuation_is_unchecked_and_preserves_indent() {
        assert_eq!(continuation("   - [ ] awef"), Some((9, "   - [ ] ".into())));
        assert_eq!(continuation("  * [X] done"), Some((8, "  * [ ] ".into())));
        assert_eq!(continuation("- [x] done"), Some((6, "- [ ] ".into())));
        assert_eq!(continuation("- [ ] "), Some((6, "- [ ] ".into())));
    }
}
