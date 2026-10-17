#!/usr/bin/env python3
"""Real HTTP/CLI integration tests, no Qt network service required."""
import hashlib
import io
import json
import os
from pathlib import Path
import socket
import subprocess
import tarfile
import tempfile
import threading
import tomllib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ROOT = Path(__file__).resolve().parents[1]
VERSION = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))["workspace"]["package"]["version"]
USER_AGENT = f"Qit/{VERSION}"
BIN = Path(os.environ.get("QIT_CLI", ROOT / "target/debug/qit-cli"))
requests = []
counts = {}

def archive(name="bin/cmake", content=b"fixture tool\n", link=None):
    out = io.BytesIO()
    with tarfile.open(fileobj=out, mode="w") as tar:
        entry = tarfile.TarInfo(name)
        entry.mode = 0o755
        if link is not None:
            entry.type = tarfile.SYMTYPE
            entry.linkname = link
            tar.addfile(entry)
        else:
            entry.size = len(content)
            tar.addfile(entry, io.BytesIO(content))
    return out.getvalue()

NORMAL = archive()
PAYLOADS = {"normal": NORMAL, "redirect": NORMAL, "resume": NORMAL, "ignored-range": NORMAL,
            "throttle": NORMAL, "bad-hash": NORMAL, "traversal": archive("../../escape"),
            "link": archive("outside", link="../../escape"), "missing": NORMAL}

class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *args):
        pass

    def respond(self, status, body=b"", headers=None):
        self.send_response(status)
        self.send_header("Content-Length", str(len(body)))
        for key, value in (headers or {}).items():
            self.send_header(key, value)
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        requests.append((self.path, {key.lower(): value for key, value in self.headers.items()}))
        if self.headers.get("User-Agent") != USER_AGENT:
            return self.respond(403)
        parts = self.path.strip("/").split("/")
        mode = parts[0]
        if mode not in PAYLOADS:
            return self.respond(404)
        body = PAYLOADS[mode]
        if self.path.endswith("Updates.xml"):
            xml = f"""<Updates><PackageUpdate><Name>qt.tools.cmake</Name>
<DisplayName>CMake fixture</DisplayName><Version>1.0.0</Version>
<DownloadableArchives>fixture.tar</DownloadableArchives>
<Operations><Operation name="Extract"><Argument>@TargetDir@/Tools/CMake</Argument><Argument>fixture.tar</Argument></Operation></Operations>
</PackageUpdate></Updates>"""
            return self.respond(200, xml.encode())
        if self.path.endswith(".sha256"):
            if mode == "missing":
                return self.respond(404)
            value = "0" * 64 if mode == "bad-hash" else hashlib.sha256(body).hexdigest()
            return self.respond(200, value.encode())
        if self.path.endswith(".sha1"):
            return self.respond(404)
        if not self.path.endswith("fixture.tar"):
            return self.respond(404)
        count = counts.get(mode, 0)
        counts[mode] = count + 1
        if mode == "redirect" and "redirected" not in self.path:
            return self.respond(302, headers={"Location": "/redirect/redirected/fixture.tar"})
        if mode == "throttle" and count == 0:
            return self.respond(429, headers={"Retry-After": "0"})
        if mode in ("resume", "ignored-range") and count == 0:
            self.send_response(200)
            self.send_header("Content-Length", str(len(body)))
            self.send_header("ETag", '"fixture-v1"')
            self.end_headers()
            self.wfile.write(body[:2048])
            self.wfile.flush()
            self.connection.shutdown(socket.SHUT_RDWR)
            self.connection.close()
            return
        if mode == "resume" and self.headers.get("Range"):
            assert self.headers.get("If-Range") == '"fixture-v1"'
            start = int(self.headers["Range"].removeprefix("bytes=").rstrip("-"))
            return self.respond(206, body[start:], {"Content-Range": f"bytes {start}-{len(body)-1}/{len(body)}", "ETag": '"fixture-v1"'})
        return self.respond(200, body, {"ETag": '"fixture-v1"'})

def main():
    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    with tempfile.TemporaryDirectory(prefix="qit-protocol-") as work:
        base = Path(work)
        env = dict(os.environ, QIT_DATA_DIR=str(base / "data"), NO_PROXY="127.0.0.1", no_proxy="127.0.0.1")

        def cli(*args, ok=True):
            result = subprocess.run([str(BIN), *map(str, args)], env=env, text=True, capture_output=True, timeout=40)
            assert (result.returncode == 0) == ok, result.stderr
            return json.loads(result.stdout) if ok else result.stderr

        mirror = f"http://127.0.0.1:{server.server_port}/"
        for mode in PAYLOADS:
            success = mode not in ("bad-hash", "traversal", "link", "missing")
            result = cli("--mirror", mirror, "install", "--repo", mode, "--root", base / "安装 with spaces", "--name", mode, "--accept-licenses", "qt.tools.cmake", ok=success)
            target = base / "安装 with spaces" / mode
            if success:
                assert (target / "Tools/CMake/bin/cmake").read_bytes() == b"fixture tool\n"
                if mode == "normal":
                    (target / "Tools/CMake/bin/cmake").write_text("modified")
                    (target / "extra.txt").write_text("user file")
                preserved = cli("uninstall", result["id"], "--yes")
                if mode == "normal":
                    assert preserved and (target / "extra.txt").exists()
                    assert (target / "Tools/CMake/bin/cmake").read_text() == "modified"
                else:
                    assert not target.exists(), target
            else:
                assert not target.exists(), result
            print(f"PASS {mode}")
        assert not list((base / "安装 with spaces").glob(".qit-stage-*"))
        assert cli("list") == []
        assert any(headers.get("range") for path, headers in requests if path.startswith("/resume/"))
        assert all(headers.get("user-agent") == USER_AGENT for _, headers in requests)
        assert len(requests) > 20
        print(f"PASS {len(requests)} HTTP requests used the exact product UA; no fallback requests")
    server.shutdown()

if __name__ == "__main__":
    main()
