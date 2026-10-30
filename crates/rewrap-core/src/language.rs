//! The language table: pure data, generated at build time from `languages/*.json`.
//!
//! Nothing in this module names a language. [`Language::for_file`] is the only
//! lookup, and it matches the editor's language id or the file's extension
//! against the generated table. That is what keeps adding a language a data
//! change: there is no `match` to extend and no module to add.

/// The shape of a comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A marker repeated at the start of each line: `//`, `#`, `--`, `;`, `%`.
    Line,
    /// A start and end marker, possibly spanning lines: `/* */`, `(* *)`.
    Delimited,
    /// A quoted string used as documentation: Python's `"""`.
    String,
}

/// How the text inside a comment is read.
///
/// Closed on purpose. A dialect is the expensive axis -- days, not minutes --
/// and pretending otherwise is how "cheap to add a language" turns into a
/// parser project.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    /// Paragraphs, lists, quotes, fenced blocks left alone.
    Markdown,
    /// Markdown, with `@param`-style tags starting their own block.
    JsDoc,
    /// Markdown, with XML tag lines frozen.
    XmlDoc,
    /// Markdown, with `@param`-style tags starting their own block.
    Javadoc,
    /// Markdown, with `\param`-style commands starting their own block.
    Doxygen,
    /// Go doc: a line indented by a space or tab is code.
    Godoc,
    /// Markdown, with indented code.
    RustDoc,
    /// Markdown, with `{@template}`-style tags frozen.
    Dartdoc,
    /// Plain text: paragraphs separated by blank lines.
    PlainText,
    /// A documentation string: plain text with doctests, literal blocks and
    /// directives frozen.
    Docstring,
    /// Never reflowed.
    Verbatim,
}

/// One way of writing a comment in a language.
#[derive(Debug, Clone, Copy)]
pub struct Comment {
    pub kind: Kind,
    pub start: &'static str,
    pub end: &'static str,
    /// Whether `start` and `end` nest (Rust `/* */`, OCaml `(* *)`).
    pub nesting: bool,
    /// A character a block-comment body line conventionally carries before its
    /// text (`*` in `/* * text */`), re-emitted on every new line.
    pub decorate: &'static str,
    pub dialect: Dialect,
    /// For [`Kind::String`]: leave a string that opens and closes on one line
    /// alone, because a newline inside it changes the value.
    pub skip_single_line: bool,
    /// Reject the marker when it is followed by this character, for `#!`.
    pub not_followed_by: &'static str,
}

/// One language: a name, the ids and extensions it answers to, and its comment
/// shapes.
#[derive(Debug, Clone, Copy)]
pub struct Language {
    pub name: &'static str,
    pub id: &'static str,
    pub ext: &'static [&'static str],
    /// Set for a prose file, where the whole document is content.
    pub document: Option<Dialect>,
    pub comments: &'static [Comment],
}

include!(concat!(env!("OUT_DIR"), "/languages_gen.rs"));

impl Language {
    /// The language for an editor language id, falling back to the file name.
    ///
    /// The id wins when the editor knows the file: it already decided that a
    /// `.h` is C or C++, and second-guessing it from the extension would be
    /// worse than trusting it. The path is consulted when the id is missing or
    /// generic, which is the only way to identify a `Makefile` or a
    /// `Dockerfile` -- they have no extension at all.
    #[must_use]
    pub fn for_file(language: &str, path: &str) -> Option<&'static Language> {
        let id = language.trim().to_ascii_lowercase();
        // Every editor reports one id for "a file I do not recognise", and it is
        // the id of the fallback language. Treating it as a decision would stop
        // the extension from ever being consulted, which is how a `.md` opened
        // as an unknown file would stop being wrapped as Markdown.
        let is_default_id = LANGUAGES
            .iter()
            .any(|language| language.document.is_some() && language.id == id);
        if !id.is_empty() && !is_default_id {
            if let Some(found) = LANGUAGES.iter().find(|candidate| {
                candidate.id == id
                    || candidate.name.eq_ignore_ascii_case(&id)
                    || candidate.id.eq_ignore_ascii_case(&id)
            }) {
                return Some(found);
            }
        }
        let name = path
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(path)
            .to_ascii_lowercase();
        LANGUAGES.iter().find(|candidate| {
            candidate.ext.iter().any(|ext| {
                let ext = ext.to_ascii_lowercase();
                name.ends_with(&ext) || name == ext
            })
        })
    }

    /// Whether this is a prose file rather than code with comments.
    #[must_use]
    pub fn is_document(&self) -> bool {
        self.document.is_some()
    }
}

/// Where a string-comment opener begins on a line, if this line opens one.
///
/// A Python docstring may start after an assignment (`x = """`), so the opener
/// is searched for anywhere. The characters immediately before it must be a
/// legal string prefix (`r`, `f`, `b`, `u` and their compounds) or nothing, so
/// that a line of prose containing `"""` is not mistaken for an opener.
pub(crate) fn string_opener(line: &str, comment: &Comment) -> Option<usize> {
    let mut from = 0usize;
    while let Some(offset) = line[from..].find(comment.start) {
        let at = from + offset;
        let before = &line[..at];
        let prefix = before.trim_end();
        let tail: String = prefix
            .chars()
            .rev()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        let legal_prefix = tail.len() <= 2 && tail.chars().all(|c| "rRbBfFuU".contains(c));
        let preceded_by_word = before
            .chars()
            .rev()
            .nth(tail.len())
            .is_some_and(|c| c.is_alphanumeric() || c == '_');
        if legal_prefix && !preceded_by_word {
            return Some(at);
        }
        from = at + comment.start.len();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_language_has_a_name_and_something_to_parse() {
        for language in LANGUAGES {
            assert!(!language.name.is_empty());
            assert!(
                language.is_document() || !language.comments.is_empty(),
                "{} has neither comments nor a document dialect",
                language.name
            );
        }
    }

    #[test]
    fn ids_and_extensions_are_unique() {
        let mut ids = std::collections::BTreeSet::new();
        let mut exts = std::collections::BTreeSet::new();
        for language in LANGUAGES {
            assert!(ids.insert(language.id), "duplicate id {}", language.id);
            for ext in language.ext {
                assert!(exts.insert(*ext), "duplicate extension {ext}");
            }
        }
    }

    #[test]
    fn lookup_prefers_the_editor_id_and_falls_back_to_the_path() {
        assert_eq!(
            Language::for_file("rust", "x.txt").map(|l| l.name),
            Some("Rust")
        );
        // `plaintext` is the editor's "I don't know", so the path decides.
        assert_eq!(
            Language::for_file("plaintext", "notes.md").map(|l| l.name),
            Some("Markdown")
        );
        assert!(Language::for_file("", "x.nothing-known").is_none());
    }

    #[test]
    fn a_string_opener_may_follow_an_assignment_but_not_a_word() {
        let python = Language::for_file("python", "x.py").expect("python");
        let comment = python
            .comments
            .iter()
            .find(|c| c.kind == Kind::String)
            .expect("string comment");
        assert_eq!(string_opener("x = \"\"\"", comment), Some(4));
        assert_eq!(string_opener("f\"\"\"", comment), Some(1));
        assert_eq!(string_opener("word\"\"\"", comment), None);
    }
}
