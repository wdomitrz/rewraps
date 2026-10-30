//! The rule that keeps languages cheap.
//!
//! Adding a language must be a data change. The moment `src/` contains a branch
//! on a language name, the next language costs a code review instead of a JSON
//! file -- and the one after that costs an argument about where the special case
//! belongs. So this test fails the build if a language name appears in the
//! engine at all.
//!
//! Names are matched as whole words, case-insensitively, and only outside
//! comments: a doc comment may legitimately *mention* Python when explaining why
//! a docstring is a string rather than a comment.

use std::path::{Path, PathBuf};

fn source_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
    {
        let path = entry.path();
        if path.is_dir() {
            source_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// Production code only: a `#[cfg(test)]` module may name languages freely,
/// because a test that spells out "rust" is not a branch in the product.
fn outside_tests(text: &str) -> &str {
    match text.find("#[cfg(test)]") {
        Some(at) => &text[..at],
        None => text,
    }
}

/// Strip comments so prose about a language is allowed.
fn code_only(text: &str) -> String {
    let mut out = String::new();
    let mut in_block = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("/*") {
            in_block = true;
        }
        if in_block {
            if trimmed.contains("*/") {
                in_block = false;
            }
            continue;
        }
        match line.find("//") {
            Some(at) if !line.contains("://") => out.push_str(&line[..at]),
            _ => out.push_str(line),
        }
        out.push('\n');
    }
    out
}

/// Every string literal in the code.
///
/// The literal is the presence test that matters: a per-language branch has to
/// name the language somewhere, and in this crate the only place it could name
/// it is a string. A module called `markdown` or a variant called
/// `Dialect::Markdown` is the dialect *engine*, shared by every language in the
/// same way, and is deliberately not an offence.
fn literals(code: &str) -> Vec<String> {
    let mut out = Vec::new();
    let chars: Vec<char> = code.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '"' {
            let start = i + 1;
            let mut end = start;
            while end < chars.len() && chars[end] != '"' {
                if chars[end] == '\\' {
                    end += 1;
                }
                end += 1;
            }
            if end < chars.len() {
                out.push(chars[start..end].iter().collect());
            }
            i = end + 1;
        } else {
            i += 1;
        }
    }
    out
}

#[test]
fn no_language_name_appears_in_the_engine() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    source_files(&root, &mut files);
    assert!(
        !files.is_empty(),
        "found no sources under {}",
        root.display()
    );

    let names: Vec<String> = rewrap_core::languages()
        .into_iter()
        .chain(rewrap_core::language_ids())
        .map(str::to_ascii_lowercase)
        .collect();

    let mut offences = Vec::new();
    for file in &files {
        let text = std::fs::read_to_string(file).expect("read source");
        let code = code_only(outside_tests(&text));
        for literal in literals(&code) {
            let lower = literal.to_ascii_lowercase();
            if names.contains(&lower) {
                offences.push(format!("{} names the language {literal:?}", file.display()));
            }
        }
    }
    assert!(
        offences.is_empty(),
        "a language name in the engine means a special case that belongs in \
         languages/*.json:\n  {}",
        offences.join("\n  ")
    );
}
