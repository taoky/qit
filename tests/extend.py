#!/usr/bin/env python3
"""Exercise component additions, user-file preservation and interrupted swap recovery."""
import hashlib
import io
import json
import os
import shutil
from pathlib import Path
import subprocess
import tarfile
import tempfile
import threading
import tomllib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ROOT = Path(__file__).resolve().parents[1]
VERSION = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))["workspace"]["package"]["version"]
USER_AGENT = f"Qit/{VERSION}"
BIN = Path(os.environ.get('QIT_CLI', ROOT / 'target/debug/qit-cli'))

def archive(entries):
    out = io.BytesIO()
    with tarfile.open(fileobj=out, mode='w') as tar:
        for name, content in entries.items():
            info = tarfile.TarInfo(name)
            info.size = len(content)
            info.mode = 0o644
            tar.addfile(info, io.BytesIO(content))
    return out.getvalue()

payloads = {
    'base': archive({'base.txt': b'original', 'deleted.txt': b'delete me', 'shared.txt': b'base'}),
    'addon': archive({'addon.txt': b'new module', 'shared.txt': b'addon'}),
    'conflict': archive({'base.txt': b'conflicting module'}),
    'broken': b'not an archive',
}
requests = []
class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args): pass
    def do_GET(self):
        requests.append(self.path)
        assert self.headers.get('User-Agent') == USER_AGENT
        if self.path == '/repo/Updates.xml':
            body = '<Updates>' + ''.join(
                f'<PackageUpdate><Name>qt.qt6.681.{name}</Name><Version>6.8.1</Version>'
                f'<DownloadableArchives>{name}.tar</DownloadableArchives></PackageUpdate>'
                for name in payloads) + '</Updates>'
            body = body.encode()
        else:
            name = self.path.split('/')[-2].split('.')[-1]
            data = payloads.get(name)
            if data is None:
                self.send_error(404)
                return
            body = hashlib.sha256(data).hexdigest().encode() if self.path.endswith('.sha256') else data
        self.send_response(200)
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)

with tempfile.TemporaryDirectory(prefix='qit-extend-') as tmp:
    # Match Qit's canonical journal paths (e.g. /var -> /private/var on macOS).
    tmp = Path(tmp).resolve()
    server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    env = dict(os.environ, QIT_DATA_DIR=str(tmp / 'data'))
    root = tmp / 'sdks'
    sdk = root / 'sdk'
    mirror = f'http://127.0.0.1:{server.server_port}/'
    def run(args, ok=True):
        result = subprocess.run([str(BIN), '--mirror', mirror, *args], env=env,
                                capture_output=True, text=True, timeout=30)
        assert (result.returncode == 0) == ok, result.stdout + result.stderr
        return result
    def install(package, add=False, ok=True):
        return run(['install', '--repo', 'repo/', '--root', str(root), '--name', 'sdk',
                    '--accept-licenses', *(['--add-components'] if add else []),
                    f'qt.qt6.681.{package}'], ok)
    try:
        old = json.loads(install('base').stdout)
        (sdk / 'base.txt').write_text('user edit')
        (sdk / 'deleted.txt').unlink()
        (sdk / 'user.txt').write_text('user addition')
        (sdk / 'empty-user-dir').mkdir()
        if os.name != 'nt':
            (tmp / 'external').write_text('external user data')
            (sdk / 'user-link').symlink_to(tmp / 'external')
        updated = json.loads(install('addon', add=True).stdout)
        assert updated['id'] == old['id']
        assert {p['id'] for p in updated['plan']['packages']} == {'qt.qt6.681.base', 'qt.qt6.681.addon'}
        assert (sdk / 'base.txt').read_text() == 'user edit'
        assert not (sdk / 'deleted.txt').exists()
        assert (sdk / 'user.txt').read_text() == 'user addition'
        assert (sdk / 'empty-user-dir').is_dir()
        if os.name != 'nt':
            assert (sdk / 'user-link').is_symlink()
            assert (tmp / 'external').read_text() == 'external user data'
        assert (sdk / 'addon.txt').read_text() == 'new module'
        # Lexical archive order is deterministic; no newly introduced overwrite policy here.
        assert (sdk / 'shared.txt').read_text() == 'base'
        assert not any(f['path'] == 'user.txt' for f in updated['files'])
        assert sum(p.endswith('base.tar') for p in requests) == 1, requests
        assert 'already installed' in install('addon', add=True, ok=False).stderr
        assert 'user-modified' in install('conflict', add=True, ok=False).stderr
        install('broken', add=True, ok=False)
        assert not list(root.glob('.qit-stage-*'))
        assert not list((tmp / 'data/transactions').glob('*.json'))
        assert (sdk / 'base.txt').read_text() == 'user edit'
        assert json.loads(run(['list']).stdout) == [updated]
        # Simulate interruption at each directory-swap phase, then recover.
        for phase in ['prepared', 'old-renamed', 'new-renamed', 'record-written']:
            stage = root / f".qit-stage-{old['id']}"
            backup = root / f".qit-backup-{old['id']}"
            stage.mkdir()
            (stage / '.qit-owner').write_text(old['id'])
            (stage / 'incomplete').write_text('partial')
            journal = tmp / 'data/transactions' / f"{old['id']}.json"
            journal.write_text(json.dumps({'id': old['id'], 'root': str(root),
                                          'destination': str(sdk), 'previous': updated}))
            if phase != 'prepared': sdk.rename(backup)
            if phase in ('new-renamed', 'record-written'): stage.rename(sdk)
            if phase == 'record-written':
                record = tmp / 'data/installations' / f"{old['id']}.json"
                record.write_text(json.dumps(old))
            run(['recover', '--root', str(root)])
            assert (sdk / 'addon.txt').read_text() == 'new module', phase
            assert (sdk / 'base.txt').read_text() == 'user edit', phase
            assert json.loads(run(['list']).stdout) == [updated], phase
            assert not stage.exists() and not backup.exists() and not journal.exists()
        run(['uninstall', old['id'], '--yes'])
        assert (sdk / 'base.txt').read_text() == 'user edit'
        assert (sdk / 'user.txt').read_text() == 'user addition'
        assert not (sdk / 'addon.txt').exists()
        shutil.rmtree(sdk)
        for remove_parent in (False, True):
            missing = json.loads(install('base').stdout)
            record = tmp / 'data/installations' / f"{missing['id']}.json"
            # A missing owner marker in an existing directory is still an error.
            owner = sdk / '.qit-owner'
            owner.unlink()
            run(['uninstall', missing['id'], '--yes'], ok=False)
            assert record.exists() and (sdk / 'base.txt').exists()
            if os.name != 'nt':
                # A dangling symlink must not be mistaken for a deleted directory.
                shutil.rmtree(sdk)
                sdk.symlink_to(tmp / 'nonexistent', target_is_directory=True)
                run(['uninstall', missing['id'], '--yes'], ok=False)
                assert record.exists() and sdk.is_symlink()
                sdk.unlink()
            else:
                shutil.rmtree(sdk)
            if remove_parent:
                shutil.rmtree(root)
            run(['uninstall', missing['id'], '--yes'])
            assert not record.exists()
            assert json.loads(run(['list']).stdout) == []
            assert not sdk.exists()
            if remove_parent:
                assert not root.exists(), 'Removing a stale record must not recreate the SDK root'
        print('Component addition, cached reuse, conflicts, recovery and uninstall passed.')
    finally:
        server.shutdown()
