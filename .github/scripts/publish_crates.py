#!/usr/bin/env python3
"""Publish the workspace's crates to crates.io, one at a time.

Dependencies go first. A crate whose version is already on crates.io is
skipped, so a failed or interrupted release can simply be run again. When
crates.io answers "429 Too Many Requests", the script waits and retries.
"""

import json
import subprocess
import sys
import time
import urllib.error
import urllib.request

USER_AGENT = "rooste-ssh release (https://github.com/rooste-ssh/rooste-ssh)"
RETRY_WAIT = 10 * 60
ATTEMPTS = 6


def publishable_crates():
    """Publishable workspace crates, each after its workspace dependencies."""
    meta = json.loads(
        subprocess.run(
            ["cargo", "metadata", "--no-deps", "--format-version", "1"],
            check=True,
            capture_output=True,
            text=True,
        ).stdout
    )
    crates = {
        p["name"]: p
        for p in meta["packages"]
        if p.get("publish") != []  # `publish = false` shows up as []
    }
    order, seen = [], set()

    def visit(name):
        if name in seen:
            return
        seen.add(name)
        for dep in crates[name]["dependencies"]:
            # Normal and build dependencies must be published first;
            # dev-dependencies are stripped when publishing.
            if dep["name"] in crates and dep.get("kind") != "dev":
                visit(dep["name"])
        order.append(name)

    for name in sorted(crates):
        visit(name)
    return [(name, crates[name]["version"]) for name in order]


def already_published(name, version):
    request = urllib.request.Request(
        f"https://crates.io/api/v1/crates/{name}/{version}",
        headers={"User-Agent": USER_AGENT},
    )
    try:
        with urllib.request.urlopen(request, timeout=30):
            return True
    except urllib.error.HTTPError as e:
        if e.code == 404:
            return False
        raise


def publish(name):
    for attempt in range(1, ATTEMPTS + 1):
        result = subprocess.run(
            ["cargo", "publish", "-p", name], capture_output=True, text=True
        )
        sys.stdout.write(result.stdout)
        sys.stderr.write(result.stderr)
        if result.returncode == 0:
            return
        if "429" in result.stderr and attempt < ATTEMPTS:
            print(f"crates.io rate limit; retrying {name} in {RETRY_WAIT // 60} minutes")
            time.sleep(RETRY_WAIT)
            continue
        sys.exit(f"publishing {name} failed")


def main():
    for name, version in publishable_crates():
        if already_published(name, version):
            print(f"{name} {version} is already on crates.io; skipping")
            continue
        print(f"Publishing {name} {version}")
        publish(name)


if __name__ == "__main__":
    main()
