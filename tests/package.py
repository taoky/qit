#!/usr/bin/env python3
"""Portable regressions for native packaging."""
import importlib.util
import json
import locale
import os
from pathlib import Path
import sys
import tempfile
import unittest
import zipfile
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('qit_package', Path(__file__).resolve().parents[1] / 'scripts/package.py')
package = importlib.util.module_from_spec(spec)
spec.loader.exec_module(package)


class PackagingTests(unittest.TestCase):
    def test_windows_zip_with_old_dependency_timestamps(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / 'Cargo.toml').write_text('[workspace.package]\nversion = "0.2.0"\n', encoding='utf-8')
            prefix = root / 'native'
            for path in ['target/release/qit.exe', 'target/release/qit-cli.exe',
                         'data/io.github.taoky.Qit.svg', 'LICENSE', 'THIRD_PARTY.md',
                         'native/share/licenses/example/许可.txt']:
                file = root / path
                file.parent.mkdir(parents=True, exist_ok=True)
                file.write_text('fixture', encoding='utf-8')
            license_file = prefix / 'share/licenses/example/许可.txt'
            empty_dir = prefix / 'share/licenses/empty'
            empty_dir.mkdir()
            for path in (license_file, empty_dir):
                os.utime(path, (0, 0))

            def run(*args, **kwargs):
                if args[:2] == ('rustc', '-vV'): return 'host: x86_64-pc-windows-gnu'
                if args[:2] == ('cargo', 'metadata'): return '{"packages": []}'
                self.fail(f'Unexpected command: {args}')

            with patch.object(package, 'ROOT', root), \
                 patch.object(package.platform, 'system', return_value='Windows'), \
                 patch.object(package.platform, 'machine', return_value='AMD64'), \
                 patch.object(package, 'windows_dependencies', return_value=[]), \
                 patch.object(package, 'run', side_effect=run), \
                 patch.object(sys, 'argv', ['package.py', '--prefix', str(prefix)]):
                package.main()

            with zipfile.ZipFile(root / 'dist/Qit-0.2.0-windows-x86_64.zip') as archive:
                self.assertIsNone(archive.testzip())
                self.assertEqual(archive.read('Qit/qit.exe'), b'fixture')
                name = 'Qit/share/licenses/example/许可.txt'
                self.assertEqual(archive.read(name), b'fixture')
                self.assertEqual(archive.getinfo(name).date_time, (1980, 1, 1, 0, 0, 0))
                empty = archive.getinfo('Qit/share/licenses/empty/')
                self.assertTrue(empty.is_dir())
                self.assertEqual(empty.date_time, (1980, 1, 1, 0, 0, 0))
                self.assertTrue(all(name.startswith('Qit/') for name in archive.namelist()))
            staged = root / '.build/Qit-0.2.0-windows-x86_64/Qit'
            self.assertEqual((staged / 'share/licenses/example/许可.txt').stat().st_mtime, 0)
            self.assertEqual((staged / 'share/licenses/empty').stat().st_mtime, 0)

    def test_utf8_output_under_windows_default_encoding(self):
        payload = json.dumps({'author': 'Emilio Cobos Álvarez'}, ensure_ascii=False)
        code = f'import sys; sys.stdout.buffer.write({payload.encode("utf-8")!r})'
        with patch.object(locale, 'getencoding', return_value='cp1252'):
            self.assertEqual(json.loads(package.run(sys.executable, '-c', code)), json.loads(payload))

    def test_macos_resources_stay_inside_contents(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / 'Cargo.toml').write_text('[workspace.package]\nversion = "0.1.0"\n', encoding='utf-8')
            prefix = root / 'brew'
            for path in ['target/release/qit', 'target/release/qit-cli',
                         'data/io.github.taoky.Qit.svg', 'LICENSE', 'THIRD_PARTY.md',
                         'brew/etc/fonts/fonts.conf']:
                file = root / path
                file.parent.mkdir(parents=True, exist_ok=True)
                file.write_text('fixture', encoding='utf-8')
            signed = []

            def run(*args, **kwargs):
                if args == ('brew', '--prefix'): return str(prefix)
                if args == ('brew', '--cellar'): return str(prefix / 'Cellar')
                if args[:2] == ('rustc', '-vV'): return 'host: aarch64-apple-darwin'
                if args[:2] == ('cargo', 'metadata'): return '{"packages": []}'
                if args[0] == 'install_name_tool': return ''
                if args[0] == 'codesign':
                    app = Path(args[-1])
                    self.assertEqual({p.name for p in app.iterdir()}, {'Contents'})
                    self.assertTrue((app / 'Contents/Resources/etc/fonts/fonts.conf').is_file())
                    self.assertTrue((app / 'Contents/Info.plist').is_file())
                    signed.append(app)
                    return ''
                if args[0] == 'hdiutil': return ''
                self.fail(f'Unexpected command: {args}')

            with patch.object(package, 'ROOT', root), \
                 patch.object(package.platform, 'system', return_value='Darwin'), \
                 patch.object(package.platform, 'machine', return_value='arm64'), \
                 patch.object(package, 'mac_dependencies', return_value=[]), \
                 patch.object(package, 'run', side_effect=run), \
                 patch.object(sys, 'argv', ['package.py']):
                package.main()
            self.assertEqual(len(signed), 1)


if __name__ == '__main__':
    unittest.main()
