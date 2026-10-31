"""Score this engine against Rewrap's published spec samples.

A development tool, not a gate: it clones Rewrap's specs into a temporary
directory at run time, so none of their content is stored in this repository.

    python3 scripts/oracle.py [path-to-cli]

Their samples encode *their* behaviour, and ours differs deliberately in places
(a comment is not re-parsed as block Markdown, single-line Python strings are not
split, a hanging indent is kept). A mismatch is therefore something to read, not
something that is automatically wrong -- but a crash, a lost word or a
non-idempotent wrap always is, and those are what the sweep in `invariants.py`
looks for.


Each sample is a two-column code block: the divider `->` sits at a fixed offset
on the line that carries it, and every other line is cut at that same offset.
`¦` marks the wrap column and is cut from both sides.
"""
import pathlib, subprocess, sys

import os
import sys

CLI = sys.argv[1] if len(sys.argv) > 1 else os.path.join(
    os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
    'target', 'release', 'rewraps')
SPECS = sys.argv[2] if len(sys.argv) > 2 else '/tmp/rewrap-specs/docs/specs'
MARK, DIV, SP = '¦', '->', '\u00b7'

def unescape(s):
    return s.replace('-\u2192', '\t').replace(SP, ' ')

def samples(path):
    settings, block, out = {'language': 'plaintext', 'tab': '4'}, [], []
    def flush():
        nonlocal block
        if block and any(MARK in l or DIV in l for l in block):
            out.append((dict(settings), block[:]))
        block = []
    for ln in path.read_text().split('\n'):
        if ln.startswith('    '):
            block.append(ln[4:]); continue
        s = ln.strip()
        if s.startswith('>'):
            flush()
            for part in s[1:].split(','):
                if ':' in part:
                    k, v = part.split(':', 1)
                    settings[k.strip().lower()] = v.strip().strip('"')
        elif s == '':
            continue
        else:
            flush()
    flush()
    return out

def two_columns(block):
    offset = next((l.index(DIV) + len(DIV) for l in block if DIV in l), None)
    if offset is None:
        return None, None
    left, right = [], []
    for l in block:
        l = l.ljust(offset)
        left.append(l[: offset - len(DIV)].rstrip())
        right.append(l[offset:].strip())
    # cut each side at its own `¦`
    def cut(lines):
        out, col = [], None
        for l in lines:
            if MARK in l:
                if col is None:
                    col = l.index(MARK)
                l = l[: l.index(MARK)]
            out.append(unescape(l).rstrip())
        return col, out
    (lcol, left2), (rcol, right2) = cut(left), cut(right)
    while left2 and left2[-1] == '':
        left2.pop()
    while right2 and right2[-1] == '':
        right2.pop()
    return (lcol if lcol is not None else rcol), (left2, right2)

LANGS = {'javascript': 'typescript', 'js': 'typescript', 'c++': 'cpp', 'batch file': 'bat'}
tot = ok = 0
per = {}
fails = []
for p in sorted(pathlib.Path(SPECS).rglob('*.md')):
    for settings, block in samples(p):
        col, both = two_columns(block)
        if not both or col is None:
            continue
        inp, exp = both
        if not inp:
            continue
        lang = settings.get('language', 'plaintext').strip('"').lower()
        lang = LANGS.get(lang, lang)
        tot += 1
        r = subprocess.run([CLI, '-c', str(col), '--lang', lang],
                           input='\n'.join(inp) + '\n', capture_output=True, text=True)
        got = r.stdout.split('\n')
        if got and got[-1] == '':
            got.pop()
        got = [l.rstrip() for l in got]
        exp = [l.rstrip() for l in exp]
        key = lang
        per.setdefault(key, [0, 0])[1] += 1
        if got == exp:
            ok += 1
            per[key][0] += 1
        else:
            fails.append((p.stem, lang, col, inp, exp, got))

print(f'{ok}/{tot} samples match Rewrap line for line')
print('\nworst languages:')
for lang, (a, b) in sorted(per.items(), key=lambda kv: (kv[1][0] / kv[1][1], -kv[1][1]))[:12]:
    print(f'  {lang:22} {a:3}/{b:3}')
print(f'\n{len(fails)} mismatches')
for f in fails[:5]:
    print(f'\n--- {f[0]}:{f[1]} col={f[2]}\n  in : {f[3]}\n  exp: {f[4]}\n  got: {f[5]}')
