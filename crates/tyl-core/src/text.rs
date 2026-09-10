//! Conservatively clean selections: preserve authored lines, unwrap only clear layout breaks.

pub fn normalize_selection(text: &str) -> String {
    let text = text
        .replace("\r\n", "\n")
        .replace(['\r', '\u{2028}'], "\n")
        .replace('\u{2029}', "\n\n")
        .replace(['\u{200b}', '\u{feff}'], "")
        .replace('\u{a0}', " ");
    let mut output = String::new();
    let mut paragraph = false;
    let mut previous = "";
    let mut fence: Option<&str> = None;
    for line in text.trim().lines() {
        let clean = line.trim_end();
        if clean.is_empty() {
            paragraph = !output.is_empty();
            continue;
        }
        let marker = ["```", "~~~"]
            .into_iter()
            .find(|marker| clean.trim_start().starts_with(marker));
        if !output.is_empty() {
            if paragraph {
                // Blank runs copied from rendered pages are commonly layout
                // artifacts. Outside code fences collapse any run to the same
                // single natural line boundary; fenced content keeps one blank
                // line because whitespace can be semantically significant.
                output.push_str(if fence.is_some() { "\n\n" } else { "\n" });
            } else if fence.is_none()
                && marker.is_none()
                && !looks_structured(clean)
                && !looks_structured(previous)
                && previous.trim_end().ends_with('\u{00ad}')
                && clean.chars().next().is_some_and(char::is_alphabetic)
            {
                // An explicit discretionary hyphen identifies a broken word.
            } else if fence.is_none() && marker.is_none() && is_layout_wrap(previous, clean) {
                if !(output.chars().last().is_some_and(is_cjk)
                    && clean.chars().next().is_some_and(is_cjk))
                {
                    output.push(' ');
                }
            } else {
                output.push('\n');
            }
        }
        let content = if fence.is_some() || looks_structured(clean) {
            clean.to_string()
        } else {
            clean.split_whitespace().collect::<Vec<_>>().join(" ")
        };
        output.push_str(&content.replace('\u{00ad}', ""));
        if line.ends_with("  ") {
            // Preserve Markdown hard-break semantics on subsequent normalization.
            output.push_str("  ");
        }
        if let Some(marker) = marker {
            match fence {
                None => fence = Some(marker),
                Some(open) if open == marker => fence = None,
                _ => {}
            }
        }
        previous = line;
        paragraph = false;
    }
    output
}

fn is_layout_wrap(previous: &str, next: &str) -> bool {
    // Markdown's two trailing spaces are an intentional hard break.
    if previous.ends_with("  ") || looks_structured(previous) || looks_structured(next) {
        return false;
    }
    let previous = previous.trim_end();
    let end = previous
        .trim_end_matches(['"', '\'', '”', '’', ')', '）', '」', '』', '》'])
        .chars()
        .last();
    if end.is_none_or(|c| ".!?。！？:：;；…".contains(c)) {
        return false;
    }
    // Short lines may be headings, poetry, addresses or deliberate paragraphs.
    // Prose ending mid-sentence followed by a continuation is likely a rendered
    // page's visual line wrap. CJK needs a lower measured-width threshold because
    // each glyph carries more information and rendered columns are often narrow.
    let width: usize = previous
        .chars()
        .map(|c| if is_cjk(c) { 2 } else { 1 })
        .sum();
    let cjk_count = previous.chars().filter(|c| is_cjk(*c)).count();
    let next_char = next.trim_start().chars().next();
    let cjk_continuation =
        cjk_count >= 12 && end.is_some_and(is_cjk) && next_char.is_some_and(is_cjk);
    let latin_continuation = width >= 24 && next_char.is_some_and(char::is_lowercase);
    let long_mixed_continuation =
        width >= 48 && next_char.is_some_and(|c| c.is_alphanumeric() || "\"'“‘（(".contains(c));
    cjk_continuation || latin_continuation || long_mixed_continuation
}

fn looks_structured(line: &str) -> bool {
    if line.starts_with(char::is_whitespace)
        || [
            "- ", "* ", "+ ", "•", "– ", "— ", "#", ">", "|", "```", "~~~",
        ]
        .iter()
        .any(|prefix| line.starts_with(prefix))
        || line.contains(['\t', '{', '}', '[', ']', '=', ';', '`'])
    {
        return true;
    }
    let after_number = line.trim_start_matches(|c: char| c.is_ascii_digit());
    after_number.len() != line.len() && after_number.starts_with(['.', ')', '、'])
}

fn is_cjk(c: char) -> bool {
    matches!(c, '\u{2e80}'..='\u{9fff}' | '\u{f900}'..='\u{faff}' | '\u{ff00}'..='\u{ffef}')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_natural_lines_and_collapses_blank_runs() {
        assert_eq!(
            normalize_selection(
                "  Good design matters. \r\nNext sentence.\r\n\r\n \t\r\nNew paragraph.  "
            ),
            "Good design matters.\nNext sentence.\nNew paragraph."
        );
    }

    #[test]
    fn handles_cjk_soft_hyphens_and_unicode_spacing() {
        assert_eq!(
            normalize_selection(" 中文\n翻译 \n\nsuper\u{00ad}\nmarket\u{a0} test\u{200b} "),
            "中文\n翻译\nsupermarket test"
        );
        assert_eq!(normalize_selection("state-of-the-art"), "state-of-the-art");
        assert_eq!(normalize_selection("\r\n \t"), "");
    }

    #[test]
    fn unwraps_long_continuations_without_touching_complete_sentences() {
        let line = "Well-designed tools should help people read with less effort while preserving";
        assert_eq!(
            normalize_selection(&format!("{line}\nthe meaning of the original text.")),
            format!("{line} the meaning of the original text.")
        );
        assert_eq!(
            normalize_selection(&format!("{line}.\nAnother paragraph begins here.")),
            format!("{line}.\nAnother paragraph begins here.")
        );
        assert_eq!(
            normalize_selection(&format!("{line}  \nthe intentional Markdown break.")),
            format!("{line}  \nthe intentional Markdown break.")
        );
    }

    #[test]
    fn unwraps_shorter_latin_and_cjk_layout_breaks() {
        assert_eq!(
            normalize_selection(
                "The selected sentence was wrapped\nin the middle by the application."
            ),
            "The selected sentence was wrapped in the middle by the application."
        );
        assert_eq!(
            normalize_selection(
                "这是一段由页面排版造成的无效截断换行\n应该在送去翻译之前自动连接。"
            ),
            "这是一段由页面排版造成的无效截断换行应该在送去翻译之前自动连接。"
        );
        assert_eq!(
            normalize_selection("第一段结束。\n\n\n第二段开始。"),
            "第一段结束。\n第二段开始。"
        );
    }

    #[test]
    fn keeps_headings_lists_poetry_and_indentation() {
        for sample in [
            "Shopping list\napples\nbananas",
            "# A heading\nA short explanation.\n- First item\n- Second item",
            "Steps:\n1. Open settings\n2. Save changes\n   Keep this indentation",
            "床前明月光\n疑是地上霜\n举头望明月\n低头思故乡",
            "const result = items.map(item => {\n  return item.value;\n});",
        ] {
            assert_eq!(normalize_selection(sample), sample);
        }
    }

    #[test]
    fn keeps_fenced_text_and_list_boundaries_even_when_lines_are_long() {
        let line = "This is a deliberately long line that must stay separate inside a fenced block";
        for sample in [
            format!("```text\n{line}\nanother line\n```"),
            format!("- {line}\n  a continued list item"),
            format!("{line}\n- a new list item"),
        ] {
            assert_eq!(normalize_selection(&sample), sample);
        }
    }

    #[test]
    fn cleans_unicode_separators_and_is_idempotent() {
        let sample = "\u{feff}Title\u{2028}Body\u{2029}Next paragraph\u{200b}";
        let normalized = normalize_selection(sample);
        assert_eq!(normalized, "Title\nBody\nNext paragraph");
        assert_eq!(normalize_selection(&normalized), normalized);
    }
}
