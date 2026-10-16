#!/usr/bin/env python3
"""Exercise GTK catalog loading and selection against a local fixture server.
Run: python3 scripts/run-test-session.py xvfb-run -a -s "-screen 0 1280x1024x24" python3 tests/ui-smoke.py
"""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ROOT = Path(__file__).resolve().parents[1]
requests = []
fail_tools_once = True

class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_GET(self):
        global fail_tools_once
        requests.append(self.path)
        assert self.headers.get('User-Agent') == 'Qit/0.1.0'
        body = None
        status = 200
        if self.path in ('/online/qtsdkrepository/linux_x64/desktop/', '/online/qtsdkrepository/linux_arm64/desktop/'):
            body = b'<a href="qt6_681/">Qt</a><a href="qt6_680/">Qt</a><a href="tools_cmake/">CMake</a>'
        elif self.path.endswith('/tools_cmake/Updates.xml'):
            if fail_tools_once:
                fail_tools_once = False
                body = b'not XML: intentional fixture error'
            else:
                body = (ROOT / 'tests/fixtures/cmake-linux.xml').read_bytes()
        elif self.path == '/online/qtsdkrepository/all_os/qt/':
            body = b'<a href="qt6_681_unix_line_endings_src/">src</a><a href="qt6_680_unix_line_endings_src/">src</a><a href="qt6_680_windows_line_endings_src/">windows src</a>'
        elif '/all_os/qt/' in self.path:
            version = '680' if 'qt6_680' in self.path else '681'
            if self.path.endswith('/Updates.xml'):
                body = f'<Updates><PackageUpdate><Name>qt.qt6.{version}.doc</Name><DisplayName>Qt documentation</DisplayName><Version>6.8.0</Version><DownloadableArchives>docs.7z</DownloadableArchives></PackageUpdate></Updates>'.encode()
            else:
                body = b'common Qt content'
        elif self.path.endswith('/Updates.xml') and '/desktop/qt6_' in self.path:
            body = (ROOT / 'tests/fixtures/qt6-linux-6.8.1.xml').read_bytes()
            if 'qt6_680' in self.path:
                body = body.replace(b'681', b'680').replace(b'6.8.1', b'6.8.0')
        else:
            status = 404
            body = b''
        self.send_response(status)
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)

with tempfile.TemporaryDirectory(prefix='qit-ui-') as tmp:
    server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    Path(tmp, 'settings.json').write_text(json.dumps({'mirror': f'http://127.0.0.1:{server.server_port}/', 'install_root': str(Path(tmp, 'SDKs'))}))
    env = dict(os.environ, QIT_DATA_DIR=tmp, QIT_UI_FIXTURE='1', LANGUAGE=os.environ.get('QIT_TEST_LANGUAGE', 'en'), LANG='C.UTF-8', GSK_RENDERER='cairo', GDK_BACKEND='x11', GTK_A11Y='none', GIO_USE_VFS='local')
    try:
        subprocess.run(['cargo', 'test', '--locked', '-p', 'qit-gtk', 'ui_tests::install_page_workflow', '--', '--ignored', '--nocapture'], cwd=ROOT, env=env, check=True, timeout=90)
        assert any('/all_os/qt/qt6_680_unix_line_endings_src/Updates.xml' in p for p in requests)
        assert not any('windows_line_endings' in p for p in requests)
        assert sum('/tools_cmake/Updates.xml' in p for p in requests) == 2
        print(f'GTK workflow passed; {len(requests)} local HTTP requests.')
    finally:
        server.shutdown()
