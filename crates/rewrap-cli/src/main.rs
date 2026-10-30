//! `rewraps` -- wrap text and comments from the command line.
//!
//! The point of shipping this rather than keeping it as a test harness: the
//! same engine the editor runs, driven over a whole tree. "Does it work in
//! comments" is a question you answer by running it over a repository and
//! reading the diff, not by opening files one at a time.
//!
//! `--check` is the form a pre-commit hook or a CI step wants: it changes
//! nothing and reports whether anything would change.

use std::io::{Read as _, Write as _};
use std::process::ExitCode;

use rewrap_core::{File, Settings};

const USAGE: &str = "\
rewraps -- hard wrap comments and text at a column

USAGE:
    rewraps [OPTIONS] [FILE...]

With no FILE, reads standard input and writes standard output.

OPTIONS:
    -c, --column N      wrap at column N (required unless --list-languages)
    -l, --lang LANG     language id; guessed from the file extension otherwise
    -t, --tab N         tab width in columns (default 4)
        --check         change nothing; exit 1 if any file would change
        --list-languages
    -h, --help

EXAMPLES:
    rewraps -c 88 --lang rust < src/main.rs
    rewraps -c 80 -i docs/*.md
";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mut column: Option<i32> = None;
    let mut language: Option<String> = None;
    let mut tab_width = 4usize;
    let mut check = false;
    let mut in_place = false;
    let mut files: Vec<String> = Vec::new();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            "--list-languages" => {
                for name in rewrap_core::languages() {
                    println!("{name}");
                }
                return ExitCode::SUCCESS;
            }
            "-c" | "--column" => match args.next().and_then(|v| v.parse().ok()) {
                Some(value) => column = Some(value),
                None => return usage_error("--column needs a number"),
            },
            "-l" | "--lang" | "--language" => match args.next() {
                Some(value) => language = Some(value),
                None => return usage_error("--lang needs a value"),
            },
            "-t" | "--tab" | "--tab-width" => match args.next().and_then(|v| v.parse().ok()) {
                Some(value) => tab_width = value,
                None => return usage_error("--tab needs a number"),
            },
            "--check" => check = true,
            "-i" | "--in-place" => in_place = true,
            other if other.starts_with('-') && other.len() > 1 => {
                return usage_error(&format!("unknown option {other}"));
            }
            other => files.push(other.to_string()),
        }
    }

    let Some(column) = column else {
        return usage_error("--column is required");
    };
    let settings = Settings { column, tab_width };

    if files.is_empty() {
        let mut input = String::new();
        if let Err(error) = std::io::stdin().read_to_string(&mut input) {
            eprintln!("rewraps: cannot read standard input: {error}");
            return ExitCode::FAILURE;
        }
        let out = wrap_text(
            &File {
                language: language.unwrap_or_default(),
                path: String::new(),
            },
            &settings,
            &input,
        );
        if check {
            return if out == input {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            };
        }
        print!("{out}");
        return ExitCode::SUCCESS;
    }

    let mut changed = false;
    let mut failed = false;
    for path in &files {
        match std::fs::read_to_string(path) {
            Ok(input) => {
                let file = File {
                    language: language.clone().unwrap_or_default(),
                    path: path.clone(),
                };
                let out = wrap_text(&file, &settings, &input);
                if out != input {
                    changed = true;
                    if check {
                        println!("would rewrap: {path}");
                    } else if in_place {
                        if let Err(error) = std::fs::write(path, &out) {
                            eprintln!("rewraps: cannot write {path}: {error}");
                            failed = true;
                        }
                    } else {
                        print!("{out}");
                    }
                } else if !check && !in_place && !out.is_empty() {
                    print!("{out}");
                }
            }
            Err(error) => {
                eprintln!("rewraps: cannot read {path}: {error}");
                failed = true;
            }
        }
    }
    if failed {
        ExitCode::FAILURE
    } else if check && changed {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

/// Wrap a whole document, keeping a trailing newline if there was one.
fn wrap_text(file: &File, settings: &Settings, input: &str) -> String {
    if input.is_empty() {
        return String::new();
    }
    let ends_with_newline = input.ends_with('\n');
    let body = input.strip_suffix('\n').unwrap_or(input);
    let lines: Vec<&str> = body.split('\n').collect();
    let edit = rewrap_core::rewrap(file, settings, &[], &lines);
    let result = if edit.is_empty() {
        lines.iter().map(|l| (*l).to_string()).collect()
    } else {
        edit.lines
    };
    let mut out = result.join("\n");
    if ends_with_newline {
        out.push('\n');
    }
    out
}

fn usage_error(message: &str) -> ExitCode {
    eprintln!("rewraps: {message}");
    let _ = std::io::stderr().write_all(USAGE.as_bytes());
    ExitCode::from(2)
}
