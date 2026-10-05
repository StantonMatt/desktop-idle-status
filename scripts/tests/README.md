Run the offline packaging regressions with:

```sh
python3 -m unittest discover -s scripts/tests -v
```

The suite creates private, temporary Git repositories under the project scratch
directory. It tests the real source-builder shell path with real Git/tar/xz and
fixture Cargo/dpkg/Lintian commands. It never signs, uploads, installs packages,
or operates KWin. All fixture repos and files are removed automatically.

Set `PPA_TEST_VENDOR=/path/to/vendor` to also validate the complete locked vendor
tree and every Wayland XML grant in the generated copyright file. Run the real
source-package build separately through `heavy`.

The initial vendor audit covered 123 crates, all 6,745 retained files, 224 named
license/notice/author files, and embedded source grants. The retained vendor orig
used for that audit has SHA-256
`82295d7df4591dd10a4c299008ed9f75dd7590496704a27c600bc562fcc308aa`.
The reviewed checksum-manifest hashes pin the exact file inventories and contents,
not just root license files. Do not regenerate them without inspecting new or
changed license/notice files, source headers, and nested third-party sources.

File-specific exceptions cover all real Wayland XML grants (Expat and X11),
SQLite and SQLCipher, SQLite Multiple Ciphers (including MD5, SHA1/SHA2, PBKDF2,
Rijndael, AEGIS, Argon2, Ascon, and miniz), musl and printf shims, tracing-core's
spin code, uds_windows' third-party source, uuid's getrandom-derived RNG, and
wayland-scanner's syn-derived token code. Root COPYRIGHT/AUTHORS/THIRDPARTYNOTICES
and LICENSE-THIRD-PARTY files are retained by the general crate generator;
source-level Rust Project and other attributions are collected as well.

The musl MIT grant was checked against the upstream v1.2.5 COPYRIGHT file;
the retained subset contains none of its separately licensed regex or platform
assembly. SQLite Multiple Ciphers' MIT grant was checked against upstream v2.3.3
(the version embedded in the amalgamation). Those texts and exact source
notices are included in `debian/copyright`; generation needs no license network
lookup. General crate stanzas precede these narrower exceptions to preserve
DEP-5's last-matching-stanza precedence.
