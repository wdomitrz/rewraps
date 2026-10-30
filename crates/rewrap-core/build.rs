//! Turns `languages/*.json` into the table the engine matches on.
//!
//! Adding a language is a data change on purpose: nothing under `src/` names a
//! language, and `tests/no_special_cases.rs` fails the build if one does. This
//! script is the only reader of the JSON, and it runs at *build* time so the
//! wasm carries a static table: no parser, no allocation at start-up, and no
//! dependency in the published crate.
//!
//! The checks here are the ones a marker table gets wrong silently -- a marker
//! that shadows a more specific one, a dialect that does not exist, two
//! languages claiming the same extension, a string comment with no
//! single-line policy. Each of those produces an editor that looks like it
//! works and a wrong result on real code, so it is a build error rather than a
//! test.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// The closed set of content dialects, mirrored by `Dialect` in `src/language.rs`.
const DIALECTS: &[(&str, &str)] = &[
    ("markdown", "Markdown"),
    ("jsdoc", "JsDoc"),
    ("xmldoc", "XmlDoc"),
    ("javadoc", "Javadoc"),
    ("doxygen", "Doxygen"),
    ("godoc", "Godoc"),
    ("rustdoc", "RustDoc"),
    ("dartdoc", "Dartdoc"),
    ("plaintext", "PlainText"),
    ("docstring", "Docstring"),
    ("verbatim", "Verbatim"),
];

const KINDS: &[&str] = &["line", "delimited", "string"];

/// Keys a language file may carry. Anything else is a typo that would silently
/// do nothing, which is the failure mode a schema is meant to remove.
const LANGUAGE_KEYS: &[&str] = &[
    "$schema", "name", "id", "ext", "comments", "document", "notes",
];
const COMMENT_KEYS: &[&str] = &[
    "kind",
    "line",
    "start",
    "end",
    "nesting",
    "decorate",
    "dialect",
    "single_line",
    "not_followed_by",
    "notes",
];

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let dir = manifest.join("../../languages");
    println!("cargo:rerun-if-changed={}", dir.display());

    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|error| panic!("read {}: {error}", dir.display()))
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .filter(|path| path.file_name().is_some_and(|name| name != "schema.json"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no language files in {}", dir.display());

    let mut languages = Vec::new();
    let mut owner: BTreeMap<String, String> = BTreeMap::new();
    let mut names: BTreeSet<String> = BTreeSet::new();

    for path in &files {
        let text = std::fs::read_to_string(path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        let json: serde_json::Value = serde_json::from_str(&text)
            .unwrap_or_else(|error| panic!("{}: not valid JSON: {error}", path.display()));
        unknown_keys(path, &json, LANGUAGE_KEYS, "language");

        let name = json
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_else(|| fail(path, "missing \"name\""))
            .to_string();
        if !names.insert(name.to_ascii_lowercase()) {
            fail(path, &format!("duplicate language name {name:?}"));
        }
        let id = json
            .get("id")
            .and_then(serde_json::Value::as_str)
            .map_or_else(
                || name.to_ascii_lowercase().replace(' ', ""),
                str::to_string,
            );
        let ext = strings(path, &json, "ext");
        for extension in &ext {
            if extension.starts_with('.') {
                if extension.len() < 2 || extension.contains(char::is_whitespace) {
                    fail(
                        path,
                        &format!("extension {extension:?} is not a usable suffix"),
                    );
                }
            } else if extension.is_empty() || extension.contains(char::is_whitespace) {
                // Anything not starting with a dot is an exact file name.
                // `CMakeLists.txt` is one of those, so a dot is not disqualifying.
                fail(path, &format!("{extension:?} is not a usable file name"));
            }
            if let Some(other) = owner.insert(extension.to_ascii_lowercase(), name.clone()) {
                fail(
                    path,
                    &format!("extension {extension:?} already claimed by {other}"),
                );
            }
        }

        // A prose file (Markdown, plain text) is content all the way down,
        // with no comment markers to find. It says so with `document` instead
        // of `comments`, and the two are mutually exclusive: a file cannot be
        // both "everything is content" and "code with comments", and accepting
        // both would leave the engine to pick.
        let document = json
            .get("document")
            .and_then(serde_json::Value::as_str)
            .map(|dialect| {
                DIALECTS
                    .iter()
                    .find(|(name, _)| *name == dialect)
                    .map(|(_, variant)| *variant)
                    .unwrap_or_else(|| fail(path, &format!("unknown document dialect {dialect:?}")))
            });
        let comments = json.get("comments").and_then(serde_json::Value::as_array);
        let mut parsed = Vec::new();
        match (document, comments) {
            (Some(_), Some(_)) => fail(path, "has both \"document\" and \"comments\"; pick one"),
            (Some(_), None) => {}
            (None, None) => fail(path, "missing \"comments\" array or \"document\" dialect"),
            (None, Some(comments)) => {
                if comments.is_empty() {
                    fail(path, "empty \"comments\" array");
                }
                for (index, comment) in comments.iter().enumerate() {
                    unknown_keys(path, comment, COMMENT_KEYS, &format!("comment #{index}"));
                    parsed.push(parse_comment(path, index, comment));
                }
                check_duplicates(path, &parsed);
            }
        }
        languages.push(Language {
            name,
            id,
            ext,
            comments: parsed,
            document,
        });
    }

    let mut out = String::from("// Generated by build.rs from languages/*.json. Do not edit.\n");
    out.push_str("pub static LANGUAGES: &[Language] = &[\n");
    for language in &languages {
        let ext: Vec<String> = language.ext.iter().map(|e| format!("{e:?}")).collect();
        let document = language
            .document
            .map_or_else(|| "None".to_string(), |d| format!("Some(Dialect::{d})"));
        let _ = writeln!(
            out,
            "    Language {{ name: {:?}, id: {:?}, ext: &[{}], document: {document}, comments: &[",
            language.name,
            language.id,
            ext.join(", ")
        );
        for comment in &language.comments {
            out.push_str("        ");
            out.push_str(&comment.to_rust());
            out.push_str(",\n");
        }
        out.push_str("    ] },\n");
    }
    out.push_str("];\n");

    let target = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR")).join("languages_gen.rs");
    std::fs::write(&target, out).unwrap_or_else(|error| panic!("write generated table: {error}"));
}

/// The engine takes the *longest* matching marker, so `///` beats `//` and
/// `/**` beats `/*` without the file having to be ordered. What cannot be
/// resolved by length is an outright duplicate: two shapes claiming `//` would
/// make the winner depend on file order, which is invisible from the outside.
fn check_duplicates(path: &Path, comments: &[Comment]) {
    for (index, earlier) in comments.iter().enumerate() {
        for later in &comments[index + 1..] {
            if let (Some(a), Some(b)) = (earlier.marker(), later.marker()) {
                if a == b && earlier.kind == later.kind {
                    fail(
                        path,
                        &format!("comment #{index} and the one after it both claim {a:?}"),
                    );
                }
            }
        }
    }
}

struct Language {
    name: String,
    id: String,
    ext: Vec<String>,
    comments: Vec<Comment>,
    document: Option<&'static str>,
}

struct Comment {
    kind: String,
    start: String,
    end: String,
    nesting: bool,
    decorate: String,
    dialect: &'static str,
    skip_single_line: bool,
    not_followed_by: String,
}

impl Comment {
    fn marker(&self) -> Option<&str> {
        (!self.start.is_empty()).then_some(self.start.as_str())
    }

    fn to_rust(&self) -> String {
        let kind = match self.kind.as_str() {
            "line" => "Line",
            "delimited" => "Delimited",
            _ => "String",
        };
        format!(
            "Comment {{ kind: Kind::{kind}, start: {:?}, end: {:?}, nesting: {}, decorate: {:?}, \
             dialect: Dialect::{}, skip_single_line: {}, not_followed_by: {:?} }}",
            self.start,
            self.end,
            self.nesting,
            self.decorate,
            self.dialect,
            self.skip_single_line,
            self.not_followed_by
        )
    }
}

fn parse_comment(path: &Path, index: usize, json: &serde_json::Value) -> Comment {
    let kind = json
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("line")
        .to_string();
    if !KINDS.contains(&kind.as_str()) {
        fail(path, &format!("comment #{index}: unknown kind {kind:?}"));
    }
    let dialect = json
        .get("dialect")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("markdown");
    let dialect = DIALECTS
        .iter()
        .find(|(name, _)| *name == dialect)
        .map(|(_, variant)| *variant)
        .unwrap_or_else(|| {
            fail(
                path,
                &format!("comment #{index}: unknown dialect {dialect:?}"),
            )
        });
    let text = |key: &str| -> String {
        json.get(key)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let (start, end) = match kind.as_str() {
        "line" => (text("line"), String::new()),
        _ => (text("start"), text("end")),
    };
    if start.is_empty() {
        fail(path, &format!("comment #{index}: missing marker"));
    }
    if kind != "line" && end.is_empty() {
        fail(path, &format!("comment #{index}: missing \"end\""));
    }
    let decorate = text("decorate");
    if decorate.chars().count() > 1 {
        fail(
            path,
            &format!("comment #{index}: \"decorate\" must be at most one character"),
        );
    }
    let policy = json.get("single_line").and_then(serde_json::Value::as_str);
    if let Some(policy) = policy {
        if !matches!(policy, "skip" | "wrap") {
            fail(
                path,
                &format!("comment #{index}: single_line must be \"skip\" or \"wrap\""),
            );
        }
    } else if kind == "string" {
        fail(
            path,
            &format!(
                "comment #{index}: a string comment must state single_line (\"skip\" or \"wrap\")"
            ),
        );
    }
    let not_followed_by = text("not_followed_by");
    if not_followed_by.chars().count() > 1 {
        fail(
            path,
            &format!("comment #{index}: \"not_followed_by\" must be at most one character"),
        );
    }
    Comment {
        kind,
        start,
        end,
        nesting: json
            .get("nesting")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        decorate,
        dialect,
        skip_single_line: policy == Some("skip"),
        not_followed_by,
    }
}

fn unknown_keys(path: &Path, json: &serde_json::Value, allowed: &[&str], what: &str) {
    let Some(object) = json.as_object() else {
        fail(path, &format!("{what} must be an object"));
    };
    for key in object.keys() {
        if !allowed.contains(&key.as_str()) {
            fail(path, &format!("{what} has unknown key {key:?}"));
        }
    }
}

fn strings(path: &Path, json: &serde_json::Value, key: &str) -> Vec<String> {
    json.get(key)
        .and_then(serde_json::Value::as_array)
        .unwrap_or_else(|| fail(path, &format!("missing {key:?} array")))
        .iter()
        .map(|value| {
            value
                .as_str()
                .unwrap_or_else(|| fail(path, &format!("{key:?} must hold strings")))
                .to_string()
        })
        .collect()
}

fn fail(path: &Path, message: &str) -> ! {
    panic!("{}: {message}", path.display())
}
