//! The public value types: what a caller hands the engine and what it gets back.

/// The file being wrapped: its editor language id and its path.
///
/// Both are needed. The id is what the editor decided the file is, and is
/// authoritative when present; the path is the fallback, and is the only thing
/// that identifies a `Makefile` or a `Dockerfile`, which have no extension.
#[derive(Debug, Clone, Default)]
pub struct File {
    pub language: String,
    pub path: String,
}

/// Everything the wrap depends on besides the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    /// The column to wrap at. Zero or less means "do not wrap", which is how
    /// unwrapping is expressed.
    pub column: i32,
    pub tab_width: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            column: 0,
            tab_width: 4,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Position {
    pub line: usize,
    pub character: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Selection {
    pub anchor: Position,
    pub active: Position,
}

impl Selection {
    #[must_use]
    pub fn cursor(line: usize, character: usize) -> Self {
        let position = Position { line, character };
        Self {
            anchor: position,
            active: position,
        }
    }
}

/// A replacement of whole lines, and where the selections should end up.
///
/// Whole lines rather than character offsets because that is the unit the
/// engine works in, and because an editor applies a line replacement far more
/// cheaply than a diff. `start_line..=end_line` is replaced by `lines`; when
/// `lines` is empty the range is deleted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Edit {
    pub start_line: usize,
    pub end_line: usize,
    pub lines: Vec<String>,
    /// When `None` the caller should leave the selections alone, which is
    /// correct whenever the edit did not change the line count.
    pub selections: Option<Vec<Selection>>,
}

impl Edit {
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty() && self.end_line == self.start_line
    }

    /// A replacement of `start..end` (end exclusive) with `lines`.
    #[must_use]
    pub fn replace(start: usize, end: usize, lines: Vec<String>) -> Self {
        Self {
            start_line: start,
            end_line: end.saturating_sub(1),
            lines,
            selections: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_edit_is_empty_and_a_replacement_is_not() {
        assert!(Edit::empty().is_empty());
        assert!(!Edit::replace(0, 1, vec!["x".into()]).is_empty());
    }

    #[test]
    fn replace_names_an_inclusive_end_line() {
        let edit = Edit::replace(2, 4, vec!["a".into(), "b".into()]);
        assert_eq!((edit.start_line, edit.end_line), (2, 3));
    }

    #[test]
    fn a_settings_default_never_wraps() {
        assert_eq!(Settings::default().column, 0);
    }
}
