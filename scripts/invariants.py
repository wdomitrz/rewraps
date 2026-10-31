"""The invariants that must hold for any input, swept over real samples.

Clones Rewrap's published specs into a temporary directory and runs every sample
through the engine, checking the four properties that are never a matter of
taste:

  * it does not crash;
  * no word is lost, invented or duplicated;
  * wrapping twice is wrapping once;
  * no line is over the column unless it holds one unbreakable word.

Usage: python3 scripts/invariants.py [path-to-cli] [path-to-specs]
"""
import collections
import pathlib
import re
import subprocess
import sys

CLI = sys.argv[1] if len(sys.argv) > 1 else 'target/release/rewraps'
SPECS = sys.argv[2] if len(sys.argv) > 2 else '/tmp/rewrap-specs/docs/specs'
MARK, DIV, SP = '¦', '->', '\u00b7'


def unescape(text):
    return text.replace('-\u2192', '\t').replace(SP, ' ')


def samples(path):
    settings, block, out = {'language': 'plaintext'}, [], []
    def flush():
        nonlocal block
        if block and any(MARK in l or DIV in l for l in block):
            out.append((dict(settings), block[:]))
        block = []
    for line in path.read_text().split('\n'):
        if line.startswith('    '):
            block.append(line[4:])
            continue
        stripped = line.strip()
        if stripped.startswith('>'):
            flush()
            for part in stripped[1:].split(','):
                if ':' in part:
                    key, value = part.split(':', 1)
                    settings[key.strip().lower()] = value.strip().strip('"')
        elif stripped == '':
            continue
        else:
            flush()
    flush()
    return out


def left_column(block):
    offset = next((l.index(DIV) for l in block if DIV in l), None)
    if offset is None:
        return None, None
    lines, column = [], None
    for line in block:
        part = line[:offset].rstrip()
        if MARK in part:
            column = column if column is not None else part.index(MARK)
            part = part[:part.index(MARK)]
        lines.append(unescape(part).rstrip())
    while lines and lines[-1] == '':
        lines.pop()
    return column, lines


def words(lines):
    return collections.Counter(w for l in lines for w in re.split(r'\W+', l) if w)


def main():
    total = 0
    problems = collections.Counter()
    examples = {}
    for path in sorted(pathlib.Path(SPECS).rglob('*.md')):
        for settings, block in samples(path):
            column, lines = left_column(block)
            if column is None or not lines:
                continue
            language = settings.get('language', 'plaintext').strip('"').lower()
            total += 1
            text = '\n'.join(lines) + '\n'
            first = subprocess.run(
                [CLI, '-c', str(column), '--lang', language],
                input=text, capture_output=True, text=True)
            where = f'{path.stem}:{language}'
            if first.returncode != 0:
                problems['crash'] += 1
                examples.setdefault('crash', (where, lines, first.stderr[:300]))
                continue
            got = first.stdout.split('\n')
            if got and got[-1] == '':
                got.pop()
            if words(got) != words(lines):
                problems['lost or added words'] += 1
                examples.setdefault('lost or added words', (where, lines, got))
                continue
            second = subprocess.run(
                [CLI, '-c', str(column), '--lang', language],
                input=first.stdout, capture_output=True, text=True)
            if second.stdout != first.stdout:
                problems['not idempotent'] += 1
                examples.setdefault('not idempotent', (where, got, second.stdout))
                continue
            over = [l for l in got if len(l) > column and len(l.split()) > 1]
            if over:
                problems['over the column'] += 1
                examples.setdefault('over the column', (where, got, over))
                continue
            problems['ok'] += 1
    print(f'{total} samples swept')
    for key, count in problems.most_common():
        print(f'  {key:22} {count}')
    for key, (where, first, second) in examples.items():
        print(f'\n--- first {key}: {where}\n  in : {first}\n  out: {second}')
    return 0 if problems['crash'] == 0 and problems['lost or added words'] == 0 else 1


if __name__ == '__main__':
    sys.exit(main())
