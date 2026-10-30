//! Character and string widths, and where a line may break.
//!
//! Two things here are not obvious and both are load-bearing:
//!
//! * A tab's width depends on the column it starts at, so width cannot be
//!   computed per character in isolation. Every function here takes the current
//!   column for that reason.
//! * East Asian text breaks between characters rather than at spaces, but some
//!   characters may not start a line and others may not end one. Getting this
//!   wrong does not wrap badly -- it produces text that looks broken to a
//!   reader of the language, which is worse than not wrapping at all.

/// Whether a character is drawn double-width in a terminal-style grid.
///
/// The ranges are the conventional East Asian Wide/Fullwidth blocks plus the
/// emoji planes. A character outside them is assumed to be one column, which is
/// wrong for some combining marks (they should be zero) but never wrong in a
/// way that changes where a line breaks by more than a column.
fn is_wide(c: char) -> bool {
    let c = c as u32;
    matches!(c,
        0x1100..=0x115F
        | 0x2E80..=0x303E
        | 0x3041..=0x33FF
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xA000..=0xA4CF
        | 0xA960..=0xA97F
        | 0xAC00..=0xD7A3
        | 0xF900..=0xFAFF
        | 0xFE10..=0xFE19
        | 0xFE30..=0xFE6F
        | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6
        | 0x1F300..=0x1F64F
        | 0x1F900..=0x1F9FF
        | 0x20000..=0x3FFFD
    )
}

/// Characters that may not begin a line: closing punctuation, small kana.
const NO_START: &str = "})]?,;:!?、。，．：；？！）］｝」』】〕〉》”’¢°′″‰℃｡､￠％・･ゝゞヽヾーァィゥェォッャュョヮヵヶぁぃぅぇぉっゃゅょゎゕゖㇰㇱㇲㇳㇴㇵㇶㇷㇸㇹㇺㇻㇼㇽㇾㇿ々〻ｧｨｩｪｫｬｭｮｯｰ";

/// Characters that may not end a line: opening punctuation.
const NO_END: &str = "([{‘“〈《「『【〔（［｛｢£¥＄￡￥＋";

impl crate::Settings {
    /// The width of one character starting at `column`.
    #[must_use]
    pub fn char_width(&self, column: usize, c: char) -> usize {
        if c == '\t' {
            let tab = self.tab_width.max(1);
            return tab - (column % tab);
        }
        if is_wide(c) {
            2
        } else {
            1
        }
    }
}

impl crate::Settings {
    /// The width of a whole string starting at `column`.
    #[must_use]
    pub fn width(&self, column: usize, text: &str) -> usize {
        let mut at = column;
        for c in text.chars() {
            at += self.char_width(at, c);
        }
        at - column
    }
}

/// Whether a line may be broken between these two characters.
///
/// Only consulted when there is no space to break at, which is the East Asian
/// case: those scripts have no inter-word spaces, so a break has to be allowed
/// between characters. The two exception lists exist because a full stop or a
/// closing bracket at the start of a line reads as an error to a native reader.
pub(crate) fn breakable_between(before: char, after: char) -> bool {
    if !(is_wide(before) || is_wide(after)) {
        return false;
    }
    if NO_START.contains(after) || NO_END.contains(before) {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use crate::Settings;

    fn settings() -> Settings {
        Settings {
            column: 80,
            tab_width: 4,
        }
    }

    #[test]
    fn tabs_advance_to_the_next_stop() {
        let s = settings();
        assert_eq!(s.width(0, "\t"), 4);
        assert_eq!(s.width(2, "\t"), 2);
        assert_eq!(s.width(4, "\t"), 4);
    }

    #[test]
    fn east_asian_text_is_double_width() {
        let s = settings();
        assert_eq!(s.width(0, "abc"), 3);
        assert_eq!(s.width(0, "再舞心現"), 8);
    }

    #[test]
    fn breaks_are_only_offered_where_a_reader_expects_them() {
        // Latin text without a space never breaks: splitting "don't" is wrong.
        assert!(!super::breakable_between('a', 'b'));
        // Han characters may break between them.
        assert!(super::breakable_between('再', '舞'));
        // But not before a character that may not open a line.
        assert!(!super::breakable_between('再', '。'));
        // Nor after one that may not close a line.
        assert!(!super::breakable_between('「', '再'));
    }
}
