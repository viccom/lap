"""Dry-run / apply: replace the standalone brand word "Lap" with "lappp" in
locale files, leaving code identifiers and URLs untouched.

Only string *values* are touched; keys are never modified. The replacement is
word-bounded and case-sensitive, so `lappp` (already correct) and paths like
`julyx10/lap` (lowercase) are unaffected.

Usage:
    python -I scripts/rebrand-locales.py          # dry run
    python -I scripts/rebrand-locales.py --apply  # write files
"""
import json
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
LOCALES_DIR = os.path.join(ROOT, "src-vite", "src", "locales")

# Korean attaches particles directly to a Latin word ("Lap이", "Lap을"), where
# `\b` does not fire because Hangul counts as a word character. Match instead
# when "Lap" is merely not glued to other Latin letters on either side.
BRAND = re.compile(r"(?<![A-Za-z])Lap(?![A-Za-z])")
APPLY = "--apply" in sys.argv


def rebrand(node, hits, path=""):
    if isinstance(node, dict):
        return {k: rebrand(v, hits, path + "/" + k) for k, v in node.items()}
    if isinstance(node, list):
        return [rebrand(v, hits, path + "/" + str(i)) for i, v in enumerate(node)]
    if isinstance(node, str) and BRAND.search(node):
        hits.append((path, node))
        return BRAND.sub("lappp", node)
    return node


total_files = 0
total_hits = 0
for name in sorted(os.listdir(LOCALES_DIR)):
    if not name.endswith(".json"):
        continue
    path = os.path.join(LOCALES_DIR, name)
    with open(path, encoding="utf-8") as fh:
        data = json.load(fh)
    hits = []
    updated = rebrand(data, hits)
    if not hits:
        continue
    total_files += 1
    total_hits += len(hits)
    print(f"{name}: {len(hits)} string(s)")
    for key, value in hits:
        print(f"   {key}: {value[:90]}")
    if APPLY:
        # Match the repo convention: 2-space indent, literal UTF-8, CRLF.
        text = json.dumps(updated, ensure_ascii=False, indent=2).replace("\n", "\r\n") + "\r\n"
        with open(path, "w", encoding="utf-8", newline="") as fh:
            fh.write(text)

print(f"\n{'APPLIED' if APPLY else 'DRY RUN'}: {total_files} files, {total_hits} strings")
