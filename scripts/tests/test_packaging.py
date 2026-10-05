#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Offline packaging regressions; only fixture repos are committed/tagged.

Run: python3 -m unittest discover -s scripts/tests -v
Optional real-tree license audit: PPA_TEST_VENDOR=/path/to/vendor
"""
from contextlib import redirect_stderr
import hashlib
from http.client import IncompleteRead
import importlib.util
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest
import xml.etree.ElementTree as ET
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
SCRATCH = Path(os.environ.get('PPA_SCRATCH_DIR', Path.home() / '.cache/agent-scratch/desktop-idle-status'))


def module(name):
    spec = importlib.util.spec_from_file_location(name, ROOT / 'scripts' / (name + '.py'))
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


def executable(path, text):
    path.write_text(text)
    path.chmod(0o755)


class SourceBuildTests(unittest.TestCase):
    def setUp(self):
        SCRATCH.mkdir(parents=True, exist_ok=True)
        self.temp = tempfile.TemporaryDirectory(prefix='packaging-test-', dir=SCRATCH)
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.repo = self.root / 'repo'
        self.repo.mkdir()
        (self.repo / 'scripts').mkdir()
        (self.repo / 'debian').mkdir()
        for name in ('build-ppa-source.sh', 'publish-ppa.sh', 'ppa-checksums.py'):
            shutil.copy2(ROOT / 'scripts' / name, self.repo / 'scripts' / name)
        for name in ('check-versions.py', 'vendor-copyright.py'):
            (self.repo / 'scripts' / name).write_text('# fixture: succeeds offline\n')
        shutil.copy2(ROOT / 'debian/changelog', self.repo / 'debian/changelog')
        package_version = subprocess.run(
            ['dpkg-parsechangelog', '-l' + str(self.repo / 'debian/changelog'), '-SVersion'],
            text=True, capture_output=True, check=True, timeout=30).stdout.strip()
        self.upstream_version = package_version.split(':', 1)[-1].split('-', 1)[0]
        self.tag = 'v' + self.upstream_version
        (self.repo / 'debian/copyright').write_text('Fixture\n')
        (self.repo / 'payload').write_text('release commit\n')
        self.bin = self.root / 'bin'
        self.bin.mkdir()
        self.env = dict(os.environ, PATH=str(self.bin) + ':' + os.environ['PATH'],
                        PPA_SCRATCH_DIR=str(self.root / 'scratch'), PPA_BUILD_BINARY='0',
                        GIT_CONFIG_NOSYSTEM='1', GIT_CONFIG_GLOBAL=os.devnull,
                        GIT_AUTHOR_DATE='1700000000 +0000', GIT_COMMITTER_DATE='1700000000 +0000')
        for key in list(self.env):
            if key.startswith('PPA_') and key not in ('PPA_SCRATCH_DIR', 'PPA_BUILD_BINARY'):
                self.env.pop(key)
        self.git('init', '-q')
        self.git('config', 'user.name', 'Fixture')
        self.git('config', 'user.email', 'fixture@example.invalid')
        self.git('add', '.')
        self.git('commit', '-qm', 'fixture release')
        self.commit = self.git('rev-parse', 'HEAD').stdout.strip()
        executable(self.bin / 'cargo', '''#!/bin/sh
set -eu
for last do :; done
mkdir -p "$last"
printf '%s\\n' "$SOURCE_DATE_EPOCH" > "$last/fixture"
''')
        executable(self.bin / 'dpkg-buildpackage', '''#!/bin/sh
set -eu
version=$(dpkg-parsechangelog -SVersion)
upstream=${version#*:}
upstream=${upstream%%-*}
touch "../desktop-idle-status_${version}.dsc"
printf 'Files:\\n 000 0 misc optional desktop-idle-status_%s.orig-vendor.tar.xz\\n' "$upstream" > "../desktop-idle-status_${version}_source.changes"
''')
        executable(self.bin / 'dpkg-source', '''#!/bin/sh
set -eu
if [ "$1" = --extract ]; then
    mkdir -p "$3/scripts"
    printf '# fixture\\n' > "$3/scripts/check-versions.py"
fi
''')
        executable(self.bin / 'lintian', '#!/bin/sh\nexit 0\n')
        for name in ('debsign', 'dput'):
            executable(self.bin / name, '#!/bin/sh\necho UNEXPECTED-PUBLISH >&2\nexit 99\n')

    def git(self, *args):
        return subprocess.run(['git', '-C', str(self.repo), *args], env=self.env,
                              text=True, capture_output=True, check=True, timeout=30)

    def run_script(self, name='build-ppa-source.sh', **env):
        return subprocess.run([str(self.repo / 'scripts' / name), str(self.root / 'output')],
                              env=dict(self.env, **env), text=True, capture_output=True, timeout=60)

    def assert_built_release(self, result):
        self.assertEqual(result.returncode, 0, result.stderr)
        archive = self.root / f'output/desktop-idle-status_{self.upstream_version}.orig.tar.xz'
        with tarfile.open(archive) as tar:
            self.assertEqual(tar.extractfile(f'desktop-idle-status-{self.upstream_version}/payload').read(), b'release commit\n')
        with tarfile.open(self.root / f'output/desktop-idle-status_{self.upstream_version}.orig-vendor.tar.xz') as tar:
            self.assertTrue(all(member.mtime == 1700000000 for member in tar.getmembers()))
            self.assertEqual(tar.extractfile('vendor/fixture').read(), b'1700000000\n')

    def test_annotated_tag_build_path(self):
        self.git('tag', '-a', self.tag, '-m', 'annotated release message')
        self.assert_built_release(self.run_script())

    def test_branch_named_like_tag_rejected(self):
        self.git('branch', self.tag)
        result = self.run_script()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(f'refs/tags/{self.tag}', result.stderr)
        self.assertFalse((self.root / f'output/desktop-idle-status_{self.upstream_version}.orig.tar.xz').exists())

    def test_tag_wins_over_same_named_branch(self):
        self.git('tag', self.tag)
        (self.repo / 'payload').write_text('branch contents\n')
        self.git('add', 'payload')
        self.git('commit', '-qm', 'branch only')
        self.git('branch', self.tag)
        self.assert_built_release(self.run_script())

    def test_ref_is_pinned_before_archive_and_timestamp_is_numeric(self):
        real_git = shutil.which('git', path=os.environ['PATH'])
        # Move the tag after rev-parse; archive and timestamp must use its old SHA.
        self.git('tag', self.tag)
        (self.repo / 'payload').write_text('new commit\n')
        self.git('add', 'payload')
        self.git('commit', '-qm', 'new commit')
        executable(self.bin / 'git', f'''#!/bin/sh
set -eu
if [ "$3" = show ]; then
    "{real_git}" -C "$2" tag -f {self.tag} HEAD >/dev/null
fi
exec "{real_git}" "$@"
''')
        self.assert_built_release(self.run_script())

    def test_invalid_timestamp_rejected_before_archive(self):
        self.git('tag', self.tag)
        real_git = shutil.which('git', path=os.environ['PATH'])
        executable(self.bin / 'git', f'''#!/bin/sh
if [ "$3" = show ]; then printf 'tag message\\n1700000000\\n'; exit 0; fi
exec "{real_git}" "$@"
''')
        result = self.run_script()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('Invalid source commit timestamp', result.stderr)

    def test_head_validation_build(self):
        self.assert_built_release(self.run_script(PPA_SOURCE_REF='HEAD'))

    def test_untracked_packaging_rejected_despite_git_config(self):
        self.git('config', 'status.showUntrackedFiles', 'no')
        (self.repo / 'debian/unreviewed').write_text('untracked packaging\n')
        result = self.run_script('publish-ppa.sh', PPA_DRY_RUN='1')
        self.assertEqual(result.returncode, 1)
        self.assertIn('clean committed tree', result.stderr)
        self.assertNotIn('UNEXPECTED-PUBLISH', result.stderr)

    def test_ignored_packaging_rejected(self):
        (self.repo / '.git/info/exclude').write_text('debian/unreviewed\n')
        (self.repo / 'debian/unreviewed').write_text('ignored packaging\n')
        result = self.run_script('publish-ppa.sh', PPA_DRY_RUN='1')
        self.assertEqual(result.returncode, 1)
        self.assertIn('clean committed tree', result.stderr)
        self.assertNotIn('UNEXPECTED-PUBLISH', result.stderr)

    def test_git_status_failure_cannot_publish(self):
        real_git = shutil.which('git', path=os.environ['PATH'])
        executable(self.bin / 'git', f'''#!/bin/sh
if [ "$3" = status ]; then echo git-status-failed >&2; exit 2; fi
exec "{real_git}" "$@"
''')
        result = self.run_script('publish-ppa.sh', PPA_DRY_RUN='1')
        self.assertEqual(result.returncode, 2)
        self.assertIn('git-status-failed', result.stderr)
        self.assertNotIn('UNEXPECTED-PUBLISH', result.stderr)

    def test_clean_publish_rejects_worktree_and_ref_overrides(self):
        for env in ({'PPA_INCLUDE_WORKTREE': '1'}, {'PPA_SOURCE_REF': 'HEAD'}):
            with self.subTest(env=env):
                result = self.run_script('publish-ppa.sh', PPA_DRY_RUN='1', **env)
                self.assertEqual(result.returncode, 1)
                self.assertNotIn('UNEXPECTED-PUBLISH', result.stderr)

    def test_worktree_listing_failure_stops_build(self):
        real_git = shutil.which('git', path=os.environ['PATH'])
        executable(self.bin / 'git', f'''#!/bin/sh
if [ "$3" = ls-files ]; then echo git-list-failed >&2; exit 2; fi
exec "{real_git}" "$@"
''')
        result = self.run_script(PPA_SOURCE_REF='HEAD', PPA_INCLUDE_WORKTREE='1')
        self.assertEqual(result.returncode, 2)
        self.assertIn('git-list-failed', result.stderr)


class RebuildTests(unittest.TestCase):
    def setUp(self):
        self.helper = module('check-kwin-rebuild')

    def test_same_upstream_and_rebuild_status(self):
        for version, expected in [('4:6.6.6-0ubuntu0.2', 0), ('4:6.7.0-1', 1)]:
            with patch.object(sys, 'argv', ['check', '--built-version', '4:6.6.6-1', '--archive-version', version]):
                self.assertEqual(self.helper.cli(), expected)

    def test_corrupt_download_returns_error_without_traceback(self):
        # Run real dpkg-deb on a malformed download, with only Launchpad mocked.
        with patch.object(sys, 'argv', ['check', '--ppa', '--archive-version', '4:6.6.6-1']), \
             patch.object(self.helper, 'binaries', return_value=[{'binary_package_version': '1', 'self_link': 'fixture'}]), \
             patch.object(self.helper, 'request', return_value=['https://example.invalid/bridge.deb']), \
             patch.object(self.helper, 'urlopen', return_value=io.BytesIO(b'not a deb')), \
             redirect_stderr(io.StringIO()) as stderr:
            self.assertEqual(self.helper.cli(), 2)
            self.assertIn('Cannot determine', stderr.getvalue())
            self.assertNotIn('Traceback', stderr.getvalue())

    def test_subprocess_and_download_failures_are_error_status(self):
        for error in (subprocess.CalledProcessError(2, ['dpkg-deb']),
                      subprocess.TimeoutExpired(['dpkg'], 30),
                      FileNotFoundError('dpkg-deb'), IncompleteRead(b'partial', 100)):
            with self.subTest(error=error), patch.object(self.helper, 'main', side_effect=error), redirect_stderr(io.StringIO()):
                self.assertEqual(self.helper.cli(), 2)

    def test_invalid_dpkg_comparison_is_an_error(self):
        entries = [{'binary_package_version': '1'}, {'binary_package_version': '2'}]
        with patch.object(self.helper.subprocess, 'run', return_value=subprocess.CompletedProcess(['dpkg'], 2)):
            with self.assertRaises(subprocess.CalledProcessError):
                self.helper.newest(entries)

    def test_comparison_false_is_normal(self):
        entries = [{'binary_package_version': '2'}, {'binary_package_version': '1'}]
        self.assertEqual(self.helper.newest(entries), entries[0])

    def test_check_versions_subprocess_error_is_concise(self):
        with tempfile.TemporaryDirectory(prefix='versions-test-', dir=SCRATCH) as run:
            executable(Path(run) / 'dpkg-parsechangelog', '#!/bin/sh\nexit 3\n')
            result = subprocess.run([sys.executable, str(ROOT / 'scripts/check-versions.py')],
                                    env=dict(os.environ, PATH=run + ':' + os.environ['PATH']),
                                    capture_output=True, text=True, timeout=30)
        self.assertEqual(result.returncode, 2)
        self.assertIn('Cannot check release versions', result.stderr)
        self.assertNotIn('Traceback', result.stderr)


class ChecksumTests(unittest.TestCase):
    def test_invalid_inputs_return_error_without_replacing_manifest(self):
        SCRATCH.mkdir(parents=True, exist_ok=True)
        for entry in (None, '', ' 000 0 misc optional missing.tar.xz',
                      ' 000 0 misc optional ../unsafe.tar.xz', ' malformed'):
            with self.subTest(entry=entry), tempfile.TemporaryDirectory(prefix='checksum-test-', dir=SCRATCH) as run:
                directory = Path(run)
                manifest = directory / 'fixture_1.SHA256SUMS'
                manifest.write_text('previous manifest\n')
                if entry is not None:
                    (directory / 'fixture_1_source.changes').write_text('Files:\n' + entry + '\n')
                result = subprocess.run([sys.executable, str(ROOT / 'scripts/ppa-checksums.py'), run, 'fixture', '1'],
                                        text=True, capture_output=True, timeout=30)
                self.assertEqual(result.returncode, 2)
                self.assertIn('Cannot generate PPA checksums', result.stderr)
                self.assertNotIn('Traceback', result.stderr)
                self.assertEqual(manifest.read_text(), 'previous manifest\n')


class VendorTests(unittest.TestCase):
    def setUp(self):
        SCRATCH.mkdir(parents=True, exist_ok=True)
        self.helper = module('vendor-copyright')
        self.temp = tempfile.TemporaryDirectory(prefix='license-test-', dir=SCRATCH)
        self.addCleanup(self.temp.cleanup)
        self.vendor = Path(self.temp.name)
        self.crate = self.vendor / 'fixture'
        self.crate.mkdir()
        (self.crate / 'Cargo.toml').write_text('[package]\nname="fixture"\nversion="1.0"\nlicense="MIT"\n')
        (self.crate / 'LICENSE').write_text('Copyright 2026 Fixture\nMIT license text\n')
        (self.crate / 'src').mkdir()
        (self.crate / 'src/lib.rs').write_text('// Copyright 2014 The Rust Project Developers\n')
        self.checksum()

    def checksum(self):
        files = {str(p.relative_to(self.crate)): hashlib.sha256(p.read_bytes()).hexdigest()
                 for p in self.crate.rglob('*') if p.is_file() and p.name != '.cargo-checksum.json'}
        manifest = self.crate / '.cargo-checksum.json'
        manifest.write_text(json.dumps({'files': files, 'package': 'fixture'}))
        self.reviewed = {'fixture': {'name': 'fixture', 'version': '1.0',
                                    'checksum_manifest': hashlib.sha256(manifest.read_bytes()).hexdigest()}}

    def test_unreviewed_nested_notice_rejected(self):
        (self.crate / 'src/NOTICE').write_text('Unreviewed third-party notice')
        with self.assertRaisesRegex(ValueError, 'Unreviewed vendor file'):
            self.helper.validate_vendor(self.vendor, self.reviewed)

    def test_changed_nested_header_rejected(self):
        (self.crate / 'src/lib.rs').write_text('// Copyright 2026 Other author\n')
        with self.assertRaisesRegex(ValueError, 'Unreviewed vendor content'):
            self.helper.validate_vendor(self.vendor, self.reviewed)

    def test_changed_manifest_also_needs_review(self):
        old = self.reviewed
        (self.crate / 'src/LICENSE').write_text('Other grant')
        self.checksum()
        with self.assertRaisesRegex(ValueError, 'Unreviewed checksum manifest'):
            self.helper.validate_vendor(self.vendor, old)

    def test_missing_reviewed_notice_rejected(self):
        (self.crate / 'LICENSE').unlink()
        with self.assertRaisesRegex(ValueError, 'Missing reviewed'):
            self.helper.validate_vendor(self.vendor, self.reviewed)

    def test_source_attribution_is_preserved(self):
        self.helper.validate_vendor(self.vendor, self.reviewed)
        self.assertIn('Copyright 2014 The Rust Project Developers', self.helper.crate_stanza(self.crate, self.vendor))

    def test_reviewed_nested_stanzas_and_license_texts(self):
        text = (ROOT / 'debian/copyright').read_text()
        for notice in ('Martin Gräßlin', 'Simon Ser', 'Ulrich Telle', 'Olivier Gay',
                       'Zetetic LLC', 'Mathijs van de Nes', 'Valentin Ochs', 'Eyal Rozenberg',
                       'Thomas Pornin', 'Daniel Dinu', 'Alex Crichton', 'Joseph Birr-Pixton'):
            self.assertIn(notice, text)
        for clause in ('Neither the name of the project', "There's ABSOLUTELY NO WARRANTY",
                       'THE SOFTWARE IS PROVIDED', 'This is free and unencumbered software'):
            self.assertIn(clause, text)

    @unittest.skipUnless(os.environ.get('PPA_TEST_VENDOR'), 'set PPA_TEST_VENDOR for full locked-tree audit')
    def test_real_locked_tree_generates_full_copyright(self):
        result = subprocess.run([sys.executable, str(ROOT / 'scripts/vendor-copyright.py'), os.environ['PPA_TEST_VENDOR']],
                                text=True, capture_output=True, timeout=60)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(result.stdout.startswith('Format:'))
        self.assertLess(result.stdout.index('Files: vendor/wayland-protocols/*'),
                        result.stdout.index('Files: vendor/wayland-protocols/protocols/staging/ext-idle-notify/'))
        self.assertIn('THIRD', result.stdout.upper())
        # Each real XML grant has its own exact text and correct MIT variant.
        vendor = Path(os.environ['PPA_TEST_VENDOR'])
        for xml in sorted(vendor.rglob('*.xml')):
            try:
                notice = ET.parse(xml).getroot().find('copyright').text or ''
            except (ET.ParseError, AttributeError):
                continue
            if 'Permission' not in notice:
                continue
            file_field = 'Files: vendor/' + str(xml.relative_to(vendor))
            stanza = result.stdout[result.stdout.index(file_field):].split('\nFiles:', 1)[0]
            normalized = ' '.join(line.strip() for line in stanza.splitlines() if line.strip() != '.')
            self.assertIn(' '.join(notice.split()), ' '.join(normalized.split()), file_field)
            license_id = 'X11' if 'Permission to use, copy' in notice else 'Expat'
            self.assertIn('License: ' + license_id + '\n', stanza)


if __name__ == '__main__':
    unittest.main()
