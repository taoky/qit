#!/usr/bin/env python3
"""Refresh the bundled Qt list from MirrorZ's published metadata, never at startup."""
import argparse
import datetime
import io
import json
import sys
import tarfile
from pathlib import Path
from urllib.parse import urljoin, urlsplit
from urllib.request import Request, urlopen

SOURCE = "https://mirrors.cernet.edu.cn/list/qt"
CONFIG = "https://raw.githubusercontent.com/mirrorz-org/mirrorz-config/master/config/mirrors.cernet.edu.cn.json"
DATA = "https://codeload.github.com/mirrorz-org/mirrorz-json-legacy/tar.gz/refs/heads/master"
DEFAULT = dict(country="China", operator="教育网联合镜像站", url="https://mirrors.cernet.edu.cn/qt/")


def fetch(url):
    with urlopen(Request(url, headers={"User-Agent": "Qit/0.1.0"}), timeout=60) as response:
        return response.read()


def collect(config, archive):
    mirrors, missing = [DEFAULT.copy(), dict(country="", operator="官方 (qt.io)", url="https://download.qt.io/")], []
    seen = {m["url"] for m in mirrors}
    # Read members directly: never extract an upstream archive to the filesystem.
    with tarfile.open(fileobj=io.BytesIO(archive), mode="r:gz") as tar:
        members = {m.name.split("/", 1)[-1]: m for m in tar.getmembers() if m.isfile()}
        for name in config["mirrors"]:
            if name == "cqu":
                continue
            member = members.get(f"data/{name}.json")
            if member is None:
                missing.append(name)
                continue
            with tar.extractfile(member) as stream:
                data = json.load(stream)
            site = data["site"]
            if site.get("disable"):
                continue
            for mirror in data["mirrors"]:
                if mirror["cname"] != "qt":
                    continue
                url = urljoin(site["url"].rstrip("/") + "/", mirror["url"]).rstrip("/") + "/"
                parsed = urlsplit(url)
                if parsed.scheme not in ("http", "https") or not parsed.hostname or parsed.username or parsed.password or parsed.query or parsed.fragment:
                    raise ValueError(f"Invalid Qt mirror URL: {url}")
                if url not in seen:
                    mirrors.append(dict(country="China", operator=site["name"], url=url))
                    seen.add(url)
    if len(mirrors) < 3:
        raise ValueError("No Qt mirrors in MirrorZ data; refusing to replace the snapshot")
    return mirrors, missing


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", type=Path, help="Local mirrorz-json-legacy tar.gz snapshot")
    parser.add_argument("--config", type=Path, help="Local CERNET MirrorZ configuration JSON")
    args = parser.parse_args()
    config = json.loads(args.config.read_bytes() if args.config else fetch(CONFIG))
    archive = args.input.read_bytes() if args.input else fetch(DATA)
    mirrors, missing = collect(config, archive)
    snapshot = dict(source=SOURCE, data_source=DATA, config_source=CONFIG,
                    fetched_at=datetime.datetime.now(datetime.timezone.utc).isoformat(),
                    missing_sites=missing, mirrors=mirrors)
    destination = Path(__file__).resolve().parents[1] / "data/mirrors.json"
    destination.write_text(json.dumps(snapshot, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    if missing:
        print("MirrorZ snapshot has no metadata for: " + ", ".join(missing), file=sys.stderr)
    print(f"Wrote {len(mirrors)} mirrors to {destination}")


if __name__ == "__main__":
    main()
