//! The fill: one paragraph's text becomes lines no wider than the column.
//!
//! The whole engine funnels through [`fill`], so the rules that make wrapping
//! feel right are enforced once:
//!
//! * a break happens at the last space before the column is exceeded, never at
//!   the first space after it;
//! * a word longer than the column is left to overflow rather than split, which
//!   is the difference between wrapping a URL and destroying it;
//! * the trailing spaces at a break are dropped, as are trailing spaces at the
//!   end of the result, so wrapping does not leave whitespace behind.

use crate::Settings;

/// Reflow `text` into lines, prefixing each with `first` and then `cont`.
///
/// `text` must not contain a newline: hard breaks are paragraph boundaries and
/// the caller splits on them before calling.
pub(crate) fn fill(
    settings: &Settings,
    text: &str,
    first: &str,
    cont: &str,
    column: usize,
) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut lines: Vec<String> = Vec::new();
    let mut prefix = first.to_string();
    let mut start = 0usize;
    let mut width = settings.width(0, &prefix);
    // Position of the last place a break is allowed, counted as "before this
    // character index". `None` means there is no candidate yet on this line.
    let mut last_break: Option<usize> = None;
    let mut i = 0usize;

    while i < chars.len() {
        let c = chars[i];
        let cw = settings.char_width(width, c);

        if i > start && break_allowed(&chars, i) {
            last_break = Some(i);
        }

        // Whitespace never causes the break on its own: it is the *next* word
        // that has to fit. Counting the space means a line that fits exactly
        // breaks one word early, which is the difference between filling to the
        // column and filling to the column minus one.
        if c == ' ' {
            i += 1;
            last_break = Some(i);
            width += cw;
            continue;
        }

        if column > 0 && width + cw > column {
            // Over the limit: break at the last candidate, if one is on this
            // line. A candidate at `start` is not one -- it would emit an empty
            // line -- so an unbreakable run is allowed to overflow.
            if let Some(at) = last_break.filter(|at| *at > start) {
                lines.push(render(&prefix, &chars[start..at]));
                // Resume scanning at the first character we kept: everything
                // from `at` up to there is the space run the break consumed,
                // and re-scanning from `next` keeps the width honest.
                let next = skip_spaces(&chars, at);
                prefix = cont.to_string();
                start = next;
                i = next;
                width = settings.width(0, &prefix);
                last_break = None;
                continue;
            }
            // No candidate: keep going. A later space may offer one, and if it
            // never does the word overflows, which is the correct outcome.
        }

        width += cw;
        i += 1;
    }

    lines.push(render(&prefix, &chars[start..]));
    lines
}

fn render(prefix: &str, chars: &[char]) -> String {
    let text: String = chars.iter().collect();
    let mut line = String::with_capacity(prefix.len() + text.len());
    line.push_str(prefix);
    line.push_str(text.trim_end());
    // A line that is only a prefix (an empty line of a comment) keeps the
    // prefix but not the space that separates it from text.
    if text.trim().is_empty() {
        prefix.trim_end().to_string()
    } else {
        line
    }
}

fn skip_spaces(chars: &[char], mut at: usize) -> usize {
    while at < chars.len() && chars[at] == ' ' {
        at += 1;
    }
    at
}

/// Whether a break is allowed immediately before index `i`.
fn break_allowed(chars: &[char], i: usize) -> bool {
    let before = chars[i - 1];
    // A break after a space is the ordinary case, and the space itself is
    // dropped by the callers below.
    if before == ' ' {
        return true;
    }
    crate::width::breakable_between(before, chars[i])
}

#[cfg(test)]
mod tests {
    use super::fill;
    use crate::Settings;

    fn settings() -> Settings {
        Settings {
            column: 20,
            tab_width: 4,
        }
    }

    fn wrap(text: &str) -> Vec<String> {
        fill(&settings(), text, "", "", 20)
    }

    #[test]
    fn breaks_at_the_last_space_that_fits() {
        assert_eq!(
            wrap("alpha beta gamma delta"),
            vec!["alpha beta gamma", "delta"]
        );
    }

    #[test]
    fn a_word_longer_than_the_column_overflows_rather_than_being_split() {
        let long = "https://example.invalid/a/very/long/path";
        assert_eq!(wrap(long), vec![long]);
    }

    #[test]
    fn trailing_spaces_are_dropped_at_every_break() {
        for line in wrap("alpha beta gamma delta epsilon") {
            assert_eq!(line, line.trim_end());
        }
    }

    #[test]
    fn the_prefix_counts_towards_the_width() {
        // 17 columns of text fit in 20; the same text under a 12-column prefix
        // does not, which is the only reason a short comment wraps at all.
        assert_eq!(
            fill(&settings(), "alpha beta gamma", "// ", "// ", 20),
            vec!["// alpha beta gamma"]
        );
        assert_eq!(
            fill(&settings(), "alpha beta gamma delta", "/// ", "/// ", 20),
            vec!["/// alpha beta gamma", "/// delta"]
        );
    }

    #[test]
    fn east_asian_text_breaks_between_characters() {
        let lines = fill(&settings(), "再舞心現張否里案", "", "", 8);
        assert_eq!(lines, vec!["再舞心現", "張否里案"]);
    }

    #[test]
    fn a_line_with_no_content_keeps_only_its_prefix() {
        // An empty line inside a comment must not leave a trailing space behind.
        assert_eq!(fill(&settings(), "", "// ", "// ", 20), vec!["//"]);
    }
}

impl crate::Settings {
    /// Fill, and say where a source character ends up.
    ///
    /// `cursor` is a character index into `text`. The returned position is the
    /// output line and the column within it (counting the prefix), placed just
    /// after the last non-space character that precedes the cursor in the
    /// source. Counting characters instead of tracking offsets is deliberate: a
    /// break consumes the run of spaces it breaks at, so the spaces themselves
    /// have no position in the output, and the character a reader would point at
    /// is the word before them.
    pub(crate) fn fill_tracked(
        &self,
        text: &str,
        first: &str,
        cont: &str,
        column: usize,
        cursor: Option<usize>,
    ) -> (Vec<String>, Option<(usize, usize)>) {
        let lines = fill(self, text, first, cont, column);
        let Some(cursor) = cursor else {
            return (lines, None);
        };
        let wanted = text
            .chars()
            .take(cursor)
            .filter(|c| !c.is_whitespace())
            .count();
        let position = locate(&lines, first, cont, wanted);
        (lines, Some(position))
    }
}

/// The position just after the `wanted`-th non-space character of the output.
///
/// Prefixes are skipped, not counted: a comment marker repeated on every line is
/// not text the cursor can be "inside", and counting it would put the cursor in
/// the marker of the last line rather than after the last word.
fn locate(lines: &[String], first: &str, cont: &str, wanted: usize) -> (usize, usize) {
    let prefix_chars = |index: usize| {
        let prefix = if index == 0 { first } else { cont };
        prefix.chars().count()
    };
    if wanted == 0 {
        let start = lines
            .first()
            .map_or(0, |_| prefix_chars(0).min(lines[0].chars().count()));
        return (0, start);
    }
    let mut seen = 0usize;
    for (index, line) in lines.iter().enumerate() {
        let skip = prefix_chars(index);
        for (at, character) in line.chars().enumerate().skip(skip) {
            if character.is_whitespace() {
                continue;
            }
            seen += 1;
            if seen == wanted {
                return (index, at + 1);
            }
        }
    }
    // Past the end: the last line, after its last character.
    let last = lines.len().saturating_sub(1);
    let column = lines.get(last).map_or(0, |line| line.chars().count());
    (last, column)
}

#[cfg(test)]
mod tracking_tests {
    use crate::Settings;

    fn settings() -> Settings {
        Settings {
            column: 20,
            tab_width: 4,
        }
    }

    #[test]
    fn the_cursor_lands_after_the_last_word_it_was_typed_behind() {
        // "alpha beta gamma delta" at column 20 breaks after "gamma"; a cursor
        // at the end of the source text belongs after "delta".
        let (lines, position) = settings().fill_tracked(
            "alpha beta gamma delta",
            "// ",
            "// ",
            20,
            Some("alpha beta gamma delta".chars().count()),
        );
        assert_eq!(lines, vec!["// alpha beta gamma", "// delta"]);
        assert_eq!(position, Some((1, "// delta".chars().count())));
    }

    #[test]
    fn a_cursor_inside_the_text_stays_with_its_word() {
        let (_, position) = settings().fill_tracked(
            "alpha beta gamma delta",
            "// ",
            "// ",
            20,
            Some("alpha be".chars().count()),
        );
        // Mid-way through "beta" ("alphabe" is seven non-space characters), so
        // the cursor sits after the seventh -- the second letter of "beta".
        assert_eq!(position, Some((0, "// alpha be".chars().count())));
        // Word-granular only when the cursor is at a word boundary.
        let (_, at_word) = settings().fill_tracked(
            "alpha beta gamma delta",
            "// ",
            "// ",
            20,
            Some("alpha beta".chars().count()),
        );
        assert_eq!(at_word, Some((0, "// alpha beta".chars().count())));
    }

    #[test]
    fn a_cursor_at_the_start_goes_to_the_start_of_the_text() {
        let (_, position) = settings().fill_tracked("alpha beta", "// ", "// ", 20, Some(0));
        assert_eq!(position, Some((0, 3)));
    }
}
