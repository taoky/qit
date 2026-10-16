#!/usr/bin/env python3
"""CI helper. Start a private bus and child inside a new mount namespace."""
import os
from pathlib import Path
import subprocess
import sys

if len(sys.argv) < 2:
    raise SystemExit("Usage: run-test-session.py COMMAND [ARGUMENT...]")
if sys.argv[1] != "--inside":
    os.execvp("bwrap", ["bwrap", "--bind", "/", "/", "--dev-bind", "/dev", "/dev",
                        "--die-with-parent", "--", sys.executable,
                        str(Path(__file__).resolve()), "--inside", *sys.argv[1:]])
bus = subprocess.Popen(["dbus-daemon", "--session", "--nofork", "--print-address=1"],
                       stdout=subprocess.PIPE, text=True)
try:
    address = bus.stdout.readline().strip()
    if not address:
        raise SystemExit("Private D-Bus daemon failed to start")
    env = dict(os.environ, DBUS_SESSION_BUS_ADDRESS=address)
    env.pop("DBUS_STARTER_ADDRESS", None)
    env.pop("DBUS_STARTER_BUS_TYPE", None)
    raise SystemExit(subprocess.call(sys.argv[2:], env=env))
finally:
    bus.terminate()
    bus.wait(timeout=10)
