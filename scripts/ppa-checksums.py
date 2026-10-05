#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Hash only this revision's changes files and the artifacts they reference."""
import hashlib
from pathlib import Path
import sys


def main():
    if len(sys.argv) != 4:
        raise ValueError('Usage: ppa-checksums.py OUTPUT_DIR SOURCE VERSION')
    output, source, version = Path(sys.argv[1]), sys.argv[2], sys.argv[3]
    changes_files = sorted(output.glob(f'{source}_{version}_*.changes'))
    if not changes_files:
        raise ValueError('No .changes files found for this source revision')
    names = set()
    for changes in changes_files:
        names.add(changes.name)
        in_files = False
        file_count = 0
        for line in changes.read_text().splitlines():
            if line == 'Files:':
                in_files = True
            elif in_files and line.startswith(' '):
                fields = line.split()
                if len(fields) != 5:
                    raise ValueError(f'Malformed Files entry in {changes.name}')
                names.add(fields[-1])
                file_count += 1
            elif in_files:
                break
        if not file_count:
            raise ValueError(f'No Files entries in {changes.name}')
    rows = []
    for name in sorted(names):
        if Path(name).name != name or name in ('.', '..'):
            raise ValueError('Unsafe artifact path in .changes')
        with (output / name).open('rb') as artifact:
            digest = hashlib.file_digest(artifact, 'sha256').hexdigest()
        rows.append(digest + '  ' + name + '\n')
    # Inspect every input before replacing an existing checksum manifest.
    (output / f'{source}_{version}.SHA256SUMS').write_text(''.join(rows))


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError) as error:
        print(f'Cannot generate PPA checksums: {error}', file=sys.stderr)
        sys.exit(2)
