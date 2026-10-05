#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Generate full DEP-5 copyright for the audited, locked vendor tree.

Review the entire tree before updating vendor-copyright-reviewed.json. Its
checksum manifests pin every file, including nested notices and source headers.
Unknown, changed, missing or added files fail before any output is produced.
File-specific third-party grants in debian/copyright follow the crate stanzas,
so their licenses override the general crate license (DEP-5 last-match rule).
"""
import hashlib
import json
from pathlib import Path
import re
import sys
import tomllib

ROOT = Path(__file__).resolve().parent.parent
NOTICE_NAME = re.compile(r"LICENSE|LICENCE|COPYING|COPYRIGHT|NOTICE|AUTHORS|UNLICENSE", re.I)
COPYRIGHT = re.compile(r"copyright\s*:?[ \t]*(?:\([cC]\)[ \t]*)?(?:[0-9]|©)", re.I)


def validate_vendor(vendor, reviewed):
    crates = sorted(vendor.iterdir())
    if {p.name for p in crates} != set(reviewed):
        raise ValueError("Unreviewed vendor crate set; audit the entire tree")
    for crate in crates:
        manifest = crate / ".cargo-checksum.json"
        expected = reviewed[crate.name]
        if hashlib.sha256(manifest.read_bytes()).hexdigest() != expected["checksum_manifest"]:
            raise ValueError(f"Unreviewed checksum manifest for {crate.name}; audit licenses and source headers")
        metadata = tomllib.loads((crate / "Cargo.toml").read_text())["package"]
        if (metadata["name"], metadata["version"]) != (expected["name"], expected["version"]):
            raise ValueError(f"Unreviewed crate version: {crate.name}")
        files = json.loads(manifest.read_text())["files"]
        actual = set()
        for path in crate.rglob("*"):
            if path.is_symlink():
                raise ValueError(f"Unreviewed vendor symlink: {path}")
            if path.is_file() and path != manifest:
                name = path.relative_to(crate).as_posix()
                actual.add(name)
                if name not in files:
                    raise ValueError(f"Unreviewed vendor file (license/notice or source): {crate.name}/{name}")
                if hashlib.sha256(path.read_bytes()).hexdigest() != files[name]:
                    raise ValueError(f"Unreviewed vendor content: {crate.name}/{name}")
        if actual != set(files):
            raise ValueError(f"Missing reviewed vendor files: {crate.name}")
    return crates


def crate_stanza(crate, vendor):
    metadata = tomllib.loads((crate / "Cargo.toml").read_text())["package"]
    license_id = metadata.get("license", "").replace("/", " or ")
    license_id = re.sub(r"\bOR\b", "or", license_id)
    license_id = re.sub(r"\bAND\b", "and", license_id)
    license_id = license_id.replace(" WITH LLVM-exception", " with LLVM exception")
    texts = sorted(p for p in crate.iterdir() if p.is_file() and NOTICE_NAME.search(p.name))
    if not texts and metadata["name"] == "rsqlite-vfs" and metadata["version"] == "0.1.1":
        # This audited workspace member inherits MIT from sqlite-wasm-rs.
        texts = [vendor / "sqlite-wasm-rs" / "LICENSE"]
    if not license_id or not texts:
        raise ValueError(f"Inspect license for {crate.name}: no declared license or shipped license text")
    license_text = "\n\n".join(p.read_text() for p in texts)
    # Include explicit source-file attributions even when the crate's root
    # license lists only its maintainer. Narrower grants follow below.
    copyrights = []
    for path in sorted(p for p in crate.rglob("*") if p.is_file()):
        for line in path.read_text(errors="replace").splitlines():
            if COPYRIGHT.search(line):
                notice = re.sub(r"^\s*(?://+|\*+)?\s*", "", line).strip()
                notice = re.sub(r"\. See the COPYRIGHT.*", "", notice)
                if notice not in copyrights:
                    copyrights.append(notice)
    if not copyrights:
        copyrights = metadata.get("authors", []) or ["The " + metadata["name"] + " contributors (see individual source files)"]
    license_text = re.sub(
        r"Apache License\s+Version 2\.0.*?END OF TERMS AND CONDITIONS",
        "On Debian systems, the full Apache License version 2.0 is available in\n"
        "/usr/share/common-licenses/Apache-2.0.", license_text, flags=re.S,
    )
    stanza = f"\nFiles: vendor/{crate.name}/*\nCopyright: " + "\n ".join(copyrights)
    stanza += "\nLicense: " + license_id + "\n"
    return stanza + "".join(" " + (line.expandtabs().rstrip() if line.strip() else ".") + "\n"
                            for line in license_text.splitlines())


def main():
    if len(sys.argv) != 2:
        raise ValueError("Usage: vendor-copyright.py VENDOR_DIR")
    vendor = Path(sys.argv[1])
    reviewed = json.loads((ROOT / "scripts/vendor-copyright-reviewed.json").read_text())
    crates = validate_vendor(vendor, reviewed)
    template = (ROOT / "debian/copyright").read_text()
    base, separator, exceptions = template.partition("\nFiles: vendor/")
    if not separator:
        raise ValueError("Missing reviewed third-party copyright stanzas")
    result = base + "".join(crate_stanza(crate, vendor) for crate in crates)
    result += separator + exceptions
    # Atomic with respect to validation: never emit a partially audited DEP-5.
    print(result, end="")


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError) as error:
        print(f"Cannot generate vendor copyright: {error}", file=sys.stderr)
        sys.exit(2)
