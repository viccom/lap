"""One-shot locale key check: report which referenced keys are missing.

Reads the keys the components reference (settings.vl.*, vl.*, aiSearch.*) and
prints, per locale file, which of them are absent. Run with `python -I`.
"""
import io
import json
import glob
import re
import os

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
LOCALES = os.path.join(ROOT, 'src', 'locales')

# Keys referenced from the Vue components that must exist everywhere.
KNOWN = {}


def collect():
    src_dir = os.path.join(ROOT, 'src')
    pattern = re.compile(r"\$t\(\s*'((?:settings\.vl|vl|aiSearch)\.[A-Za-z0-9_.]+)'")
    keys = set()
    for path in glob.glob(os.path.join(src_dir, '**', '*.vue'), recursive=True):
        with io.open(path, encoding='utf-8') as handle:
            text = handle.read()
        keys.update(pattern.findall(text))
    return keys


def flat(payload, prefix=''):
    out = {}
    if isinstance(payload, dict):
        for key, value in payload.items():
            out.update(flat(value, prefix + key + '.'))
    else:
        out[prefix[:-1]] = payload
    return out


def main():
    keys = sorted(collect())
    for path in sorted(glob.glob(os.path.join(LOCALES, '*.json'))):
        with io.open(path, encoding='utf-8') as handle:
            data = json.load(handle)
        flat_map = flat(data)
        missing = [key for key in keys if key not in flat_map]
        print(os.path.basename(path), 'referenced', len(keys), 'missing', len(missing))
        for key in missing:
            print('   ', key)


if __name__ == '__main__':
    main()
