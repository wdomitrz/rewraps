# Rewraps

Hard word wrapping for comments and prose, at the column you choose.

Put the cursor in a comment — or select text — and run **Rewraps: Wrap Selection
or Comment** from the command palette. The text is re-flowed to your column and
the comment markers are put back.

It also wraps Markdown files, plain text, and the inside of block comments,
docstrings included.

## Credit where it is due

Rewraps is an independent, from-scratch implementation **inspired by
[Rewrap](https://github.com/stkb/Rewrap) by Steve Baker**, which has done this
job well for VS Code and Visual Studio for years and whose documentation is the
best description of the problem that exists.

**No code or files from Rewrap are included here.** None of its source, its test
specifications or its data tables were copied; everything in this repository was
written for it. The overlap you can see — the same comment markers, the same
ideas about what must not be re-flowed — is the overlap of two implementations
of the same problem, not shared text. Where this behaves differently from
Rewrap, that is a deliberate choice, not an accident; see [Not the same as
Rewrap](#not-the-same-as-rewrap).

## Using it

| Command | What it does |
|---|---|
| **Rewraps: Wrap Selection or Comment** | Wraps the selection, or the comment/paragraph at the cursor |
| **Rewraps: Wrap at Column...** | Asks for a column first, then wraps |
| **Rewraps: Toggle Auto-Wrap for This File** | Turns auto-wrap on or off for the open file |

There is **no default key binding** — nothing is taken from you. To add one,
open *Keyboard Shortcuts*, search for `rewraps.wrap`, and bind it. In
`keybindings.json`:

```json
{ "key": "alt+q", "command": "rewraps.wrap",
  "when": "editorTextFocus && !editorReadonly" }
```

### Where the column comes from

Rewraps reads **`editor.rulers`**, which also draws the guide you aim at. The
first ruler is the column; a language-specific list replaces the global one, so
this is the whole configuration:

```json
{
  "editor.rulers": [88],
  "[markdown]": { "editor.rulers": [80] },
  "[python]":   { "editor.rulers": [72] }
}
```

If no ruler is set for the file, the first explicit wrap asks for a column and
remembers the answer **for that language**. There is no guessed default: wrapping
at a column nobody chose is how text ends up at 80 by accident.

Dismissing that prompt does nothing at all — no wrap, no default.

Auto-wrap never asks and never guesses: with no ruler for the file it does
nothing, and an explicit wrap asks instead.

When you type a word past the column, the text moves to the next line and **the
cursor goes with it** — it ends up after the word it pushed down, not stranded at
the end of the line that just got shorter.

### Auto-wrap

Auto-wrap defaults to **on**. It fires only when you type whitespace that pushes
the current line past the column — not on paste, undo, or a multi-cursor edit,
because an automatic rewrite that guesses wrong fights your undo history. The
status bar shows whether it is on for the open file; the toggle command
overrides the setting per file.

## Not the same as Rewrap

- **No ruler cycling.** Rewrap moves to the next ruler when you wrap twice in a
  row. Here the first ruler is the column, always: the same command should do
  the same thing every time.
- **A prompt instead of a silent 80** for an explicit wrap when no column is
  configured. Auto-wrap, which cannot prompt, uses 80.
- **A single-line Python string is never split.** Splitting `"""SELECT 1"""`
  inserts a newline into the value; only multi-line strings are wrapped.
- **Fewer settings.** No double-sentence spacing, no "reformat" mode, no
  format-on-save, no custom-language JSON.
- **Nested block comments are counted**, so `(* a (* b *) c *)` and
  `/* a /* b */ c */` end where the language says they do.

## Supported languages

57 languages, each with fixtures: Ada, Batch, C, C#, C++, CMake, CSS, Clojure, Common Lisp, D, Dockerfile, Elixir, Elm, F#, Go, GraphQL, Groovy, HTML, Haskell, INI, JSON, Java, Julia, Kotlin, LaTeX, Less, Lua, MATLAB, Makefile, Markdown, OCaml, Objective-C, PHP, Perl, PlainText, PowerShell, Prolog, Protobuf, PureScript, Python, R, Ruby, Rust, SCSS, SQL, Scala, Scheme, Shell, Svelte, Swift, TOML, TypeScript, Verilog, Vue, XAML, XML, YAML.

`cargo xtask coverage` prints the table with its fixture count, and the data
lives in [`languages/`](languages) — one JSON file per language.

A file whose language is not in the table is treated as plain text: its
paragraphs are wrapped and nothing else is touched. An unknown language never
damages a file; it just does less.

### Adding a language

Adding one is data, not code — that is a hard rule of the design, enforced by a
test that fails if a language name appears in the engine at all.

```json
{
  "$schema": "./schema.json",
  "name": "Scala",
  "ext": [".scala", ".sc"],
  "comments": [
    { "line": "///", "dialect": "markdown" },
    { "line": "//", "dialect": "markdown" },
    { "kind": "delimited", "start": "/*", "end": "*/", "nesting": true, "decorate": "*" }
  ]
}
```

1. `cargo xtask new-language scala --ext .scala --line // --block /* */`
2. Write a few cases in `tests/fixtures/scala.case`.
3. `cargo xtask bless scala` fills in the expected output — but only after the
   invariant checks pass, so it can never record damage as correct.

The build validates the file: two languages claiming one extension, a dialect
that does not exist, a string comment with no single-line policy, or a marker
that another shadows all fail the build rather than producing an editor that
looks like it works.

## The command line

The same engine, for scripts, editor plugins and pre-commit hooks:

```sh
rewraps --column 88 --lang rust < src/main.rs
rewraps --column 80 -i docs/*.md          # in place
rewraps --column 80 --check src/*.rs      # change nothing; exit 1 if it would
rewraps --list-languages
```

## Development

```sh
make check   # fmt, clippy, cargo test, wasm build, npm test
make vsix    # build the extension package
```

Nothing generated is committed: the wasm module and the bundled extension are
produced by the build, here and in CI.

The layout:

| Path | What it is |
|---|---|
| `crates/rewrap-core` | The engine. No dependencies, no I/O, no editor |
| `crates/rewrap-wasm` | The engine as a wasm module |
| `crates/rewrap-cli` | The `rewraps` command |
| `crates/xtask` | `new-language`, `bless`, `coverage` |
| `languages/` | One JSON file per language, plus the schema |
| `tests/fixtures/` | Behavioural cases, checked in both directions |
| `src/` | The VS Code extension |

## Licence

AGPL-3.0-only. See [LICENSE](LICENSE).
