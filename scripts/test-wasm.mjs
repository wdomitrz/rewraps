// Loads the built engine the way the extension does -- the generated ES module
// plus its .wasm, initialised from bytes -- and checks that one real wrap comes
// back. This is the round trip the extension depends on, and it runs without an
// editor so a broken wasm build fails `make check` instead of failing silently
// in someone's VS Code.
import {readFileSync} from 'node:fs'
import {dirname, join} from 'node:path'
import {fileURLToPath} from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const pkg = join(here, '..', 'src', 'pkg')
const engine = await import(join(pkg, 'rewrap_wasm.js'))
engine.initSync({module: readFileSync(join(pkg, 'rewrap_wasm_bg.wasm'))})

const lines = ['// alpha beta gamma delta epsilon zeta eta', 'fn main() {}']
const edit = engine.rewrap('rust', 'x.rs', 30, 4, lines, [])
if (edit.isEmpty) throw new Error('expected an edit, got none')
const got = Array.from(edit.lines)
const want = ['// alpha beta gamma delta', '// epsilon zeta eta', 'fn main() {}']
if (JSON.stringify(got) !== JSON.stringify(want)) {
  throw new Error(`wrap mismatch\n got: ${JSON.stringify(got)}\nwant: ${JSON.stringify(want)}`)
}

const languages = Array.from(engine.languages())
if (!languages.includes('Rust') || !languages.includes('OCaml')) {
  throw new Error(`language table looks wrong: ${languages.join(', ')}`)
}

const again = engine.rewrap('rust', 'x.rs', 30, 4, got, [])
if (!again.isEmpty) throw new Error('wrapping twice changed the text again')

console.log(`wasm round trip ok (${languages.length} languages)`)
