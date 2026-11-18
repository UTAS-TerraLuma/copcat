//! JSON with comments and trailing commas (JSONC), as VS Code's settings
//! files allow, turned into plain JSON for `serde_json`.
//!
//! Comments become spaces (newlines stay), so line and column numbers in a
//! parse error still point at the right place in the file.

/// `text` with `//` and `/* */` comments, trailing commas before `}` or `]`,
/// and a leading byte order mark blanked out. Strings are left alone.
pub(crate) fn to_json(text: &str) -> String {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    remove_trailing_commas(&remove_comments(text))
}

fn remove_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match (c, chars.peek()) {
            ('"', _) => {
                in_string = true;
                out.push(c);
            }
            ('/', Some('/')) => {
                out.push(' ');
                while let Some(&next) = chars.peek() {
                    if next == '\n' {
                        break;
                    }
                    out.push(' ');
                    chars.next();
                }
            }
            ('/', Some('*')) => {
                chars.next();
                out.push_str("  ");
                let mut previous = '\0';
                for next in chars.by_ref() {
                    out.push(if next == '\n' { '\n' } else { ' ' });
                    if previous == '*' && next == '/' {
                        break;
                    }
                    previous = next;
                }
            }
            _ => out.push(c),
        }
    }
    out
}

fn remove_trailing_commas(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut in_string = false;
    let mut escaped = false;
    for (at, &c) in chars.iter().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
        } else if c == '"' {
            in_string = true;
        } else if c == ',' {
            let next = chars[at + 1..].iter().find(|next| !next.is_whitespace());
            if matches!(next, Some('}' | ']')) {
                out.push(' ');
                continue;
            }
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> serde_json::Value {
        serde_json::from_str(&to_json(text)).unwrap()
    }

    #[test]
    fn line_comments_should_be_ignored() {
        let value = parse("{\n  // the theme\n  \"theme\": \"graphite\" // trailing\n}");
        assert_eq!(value["theme"], "graphite");
    }

    #[test]
    fn block_comments_should_be_ignored() {
        let value = parse("/* header\n over lines */ {\"a\": /* inline */ 1}");
        assert_eq!(value["a"], 1);
    }

    #[test]
    fn trailing_commas_should_be_ignored() {
        let value = parse("{\"a\": [1, 2, ], \"b\": {\"c\": 3,\n},\n}");
        assert_eq!(value["a"], serde_json::json!([1, 2]));
        assert_eq!(value["b"]["c"], 3);
    }

    #[test]
    fn comment_markers_inside_strings_should_be_kept() {
        let value = parse(r#"{"url": "https://example.com/a,]", "q": "say \"/*hi*/\""}"#);
        assert_eq!(value["url"], "https://example.com/a,]");
        assert_eq!(value["q"], "say \"/*hi*/\"");
    }

    #[test]
    fn a_byte_order_mark_should_be_ignored() {
        assert_eq!(parse("\u{feff}{\"a\": 1}")["a"], 1);
    }

    #[test]
    fn line_numbers_should_survive() {
        let json = to_json("// one\n/* two\nthree */\n{");
        assert_eq!(json.lines().count(), 4);
        let error = serde_json::from_str::<serde_json::Value>(&json).unwrap_err();
        assert_eq!(error.line(), 4);
    }

    #[test]
    fn an_unterminated_block_comment_should_run_to_the_end() {
        assert_eq!(to_json("{} /* never closed").trim(), "{}");
    }
}
