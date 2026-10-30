//! Repository chores.
//!
//! Three commands, each existing because the alternative is a manual step that
//! gets skipped:
//!
//! * `new-language` writes the data file and a starter fixture, so adding a
//!   language begins from something valid rather than from documentation.
//! * `bless` rewrites the expected output of a fixture from what the engine
//!   actually produces -- but only after the invariant checks pass, because a
//!   bless button that can bless corrupted output is how a fixture suite rots
//!   into a description of the bugs.
//! * `coverage` prints languages against fixtures, so "supported" stays honest.

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("new-language") => new_language(&args[1..]),
        Some("bless") => bless(&args[1..]),
        Some("coverage") => coverage(),
        _ => {
            eprintln!(
                "usage: cargo xtask <command>\n\n  \
                 new-language <name> --ext .xy [--line //] [--doc ///] [--block /* */]\n  \
                 bless [language]\n  \
                 coverage"
            );
            std::process::exit(2);
        }
    }
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn title(name: &str) -> String {
    let mut chars = name.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().collect::<String>() + chars.as_str()
    })
}

fn new_language(args: &[String]) {
    let Some(name) = args.first() else {
        eprintln!("new-language needs a name");
        std::process::exit(2);
    };
    let mut ext: Vec<String> = Vec::new();
    let mut line: Option<String> = None;
    let mut doc: Option<String> = None;
    let mut block: Option<(String, String)> = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--ext" => {
                ext.push(args.get(i + 1).cloned().unwrap_or_default());
                i += 2;
            }
            "--line" => {
                line = args.get(i + 1).cloned();
                i += 2;
            }
            "--doc" => {
                doc = args.get(i + 1).cloned();
                i += 2;
            }
            "--block" => {
                let start = args.get(i + 1).cloned().unwrap_or_default();
                let end = args.get(i + 2).cloned().unwrap_or_default();
                block = Some((start, end));
                i += 3;
            }
            _ => i += 1,
        }
    }
    if ext.is_empty() {
        eprintln!("new-language needs at least one --ext");
        std::process::exit(2);
    }
    let ext_json: Vec<String> = ext.iter().map(|e| format!("{e:?}")).collect();
    let mut comments = Vec::new();
    if let Some(doc) = &doc {
        comments.push(format!(
            r#"    {{ "line": {doc:?}, "dialect": "markdown" }}"#
        ));
    }
    if let Some(line) = &line {
        comments.push(format!(
            r#"    {{ "line": {line:?}, "dialect": "markdown" }}"#
        ));
    }
    if let Some((start, end)) = &block {
        comments.push(format!(
            r#"    {{ "kind": "delimited", "start": {start:?}, "end": {end:?}, "dialect": "markdown" }}"#
        ));
    }
    let json = format!(
        "{{\n  \"$schema\": \"./schema.json\",\n  \"name\": {:?},\n  \"ext\": [{}],\n  \"comments\": [\n{}\n  ]\n}}\n",
        title(name),
        ext_json.join(", "),
        comments.join(",\n")
    );
    let path = root().join("languages").join(format!("{name}.json"));
    if path.exists() {
        eprintln!("{} already exists", path.display());
        std::process::exit(1);
    }
    std::fs::write(&path, json).expect("write language file");

    let fixture = root().join("tests/fixtures").join(format!("{name}.case"));
    let fixture_text = format!(
        "# {}\n\n[case]\nname = a comment wraps\nlanguage = {name}\ncolumn = 30\n-- input\n{} alpha beta gamma delta epsilon\n-- output\n",
        title(name),
        line.clone().unwrap_or_else(|| "//".into())
    );
    std::fs::create_dir_all(fixture.parent().unwrap()).expect("fixtures dir");
    std::fs::write(&fixture, fixture_text).expect("write fixture");
    println!("wrote {}\nwrote {}", path.display(), fixture.display());
    println!("next: fill the expected output, or run `cargo xtask bless {name}`");
}

fn fixtures() -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect(&root().join("tests/fixtures"), &mut files);
    files.sort();
    files
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
    {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "case") {
            out.push(path);
        }
    }
}

/// Rewrite expected output from what the engine produces.
///
/// Gated on the invariant tests: blessing output from a build that corrupts text
/// would record the corruption as correct, and nobody would look again.
fn bless(filter: &[String]) {
    let gate = Command::new(env!("CARGO"))
        .args([
            "test",
            "-p",
            "rewrap-core",
            "--test",
            "fixtures",
            "preserves_every",
        ])
        .current_dir(root())
        .status();
    match gate {
        Ok(status) if status.success() => {}
        _ => {
            eprintln!(
                "refusing to bless: the invariant checks do not pass. Fix the engine \
                 first -- blessing would record the damage as expected output."
            );
            std::process::exit(1);
        }
    }

    let only = filter.first().map(|f| f.to_ascii_lowercase());
    let mut blessed = 0usize;
    for file in fixtures() {
        if let Some(only) = &only {
            if !file
                .file_name()
                .is_some_and(|n| n.to_string_lossy().to_ascii_lowercase().contains(only))
            {
                continue;
            }
        }
        let text = std::fs::read_to_string(&file).expect("read fixture");
        let (rewritten, changed) = bless_file(&text);
        if changed {
            std::fs::write(&file, rewritten).expect("write fixture");
            blessed += 1;
            println!("blessed {}", file.display());
        }
    }
    if blessed == 0 {
        println!("nothing to bless");
    }
}

/// Rewrite one fixture file's `-- output` sections.
fn bless_file(text: &str) -> (String, bool) {
    let mut out: Vec<String> = Vec::new();
    let mut changed = false;
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        if line.trim() != "[case]" {
            out.push(line.to_string());
            continue;
        }
        // Gather the case.
        let mut case: Vec<String> = vec![line.to_string()];
        while let Some(next) = lines.peek() {
            if next.trim() == "[case]" {
                break;
            }
            case.push(lines.next().unwrap().to_string());
        }
        let rendered = render_case(&case);
        changed |= rendered != case;
        out.extend(rendered);
    }
    let mut result = out.join("\n");
    if text.ends_with('\n') {
        result.push('\n');
    }
    (result, changed)
}

fn render_case(lines: &[String]) -> Vec<String> {
    let mut language = String::new();
    let mut column: i32 = 80;
    let mut tab_width = 4usize;
    let mut input: Vec<String> = Vec::new();
    let mut stage = 0;
    for line in lines {
        match line.trim() {
            "-- input" => stage = 1,
            "-- output" => stage = 2,
            _ => match stage {
                0 => {
                    if let Some((key, value)) = line.split_once('=') {
                        let value = value.trim();
                        match key.trim() {
                            "language" => language = value.to_string(),
                            "column" => column = value.parse().unwrap_or(80),
                            "tab" => tab_width = value.parse().unwrap_or(4),
                            _ => {}
                        }
                    }
                }
                1 => input.push(line.to_string()),
                _ => {}
            },
        }
    }
    let file = rewrap_core::File {
        language,
        path: String::new(),
    };
    let settings = rewrap_core::Settings { column, tab_width };
    let borrowed: Vec<&str> = input.iter().map(String::as_str).collect();
    let edit = rewrap_core::rewrap(&file, &settings, &[], &borrowed);
    let output = if edit.is_empty() {
        input.clone()
    } else {
        edit.lines
    };

    let mut out: Vec<String> = Vec::new();
    for line in lines {
        if line.trim() == "-- output" {
            break;
        }
        out.push(line.clone());
    }
    if !out.iter().any(|line| line.trim() == "-- output") {
        out.push("-- output".to_string());
    }
    out.extend(output);
    out
}

/// Languages against fixtures: the honest answer to "what is supported".
fn coverage() {
    let mut by_language: std::collections::BTreeMap<String, usize> =
        std::collections::BTreeMap::new();
    for file in fixtures() {
        let text = std::fs::read_to_string(&file).expect("read fixture");
        for line in text.lines() {
            if let Some((key, value)) = line.split_once('=') {
                if key.trim() == "language" {
                    *by_language.entry(value.trim().to_string()).or_insert(0) += 1;
                }
            }
        }
    }
    let names = rewrap_core::languages();
    let ids = rewrap_core::language_ids();
    println!("{:<16} {:<16} {:>7}", "language", "fixture id", "cases");
    for (name, id) in names.iter().zip(&ids) {
        let count = by_language.get(*id).copied().unwrap_or(0);
        let mark = if count == 0 { "  MISSING" } else { "" };
        println!("{name:<16} {id:<16} {count:>7}{mark}");
    }
    let missing: Vec<&String> = by_language
        .keys()
        .filter(|key| !ids.contains(&key.as_str()))
        .collect();
    if !missing.is_empty() {
        println!("\nfixtures naming an unknown id: {missing:?}");
    }
    let total: usize = by_language.values().sum();
    println!("\n{total} cases across {} languages", by_language.len());
}
