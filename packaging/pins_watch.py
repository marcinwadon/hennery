"""Propose one tested runtime set, including releases of transitive agent CLIs."""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import tomllib
from urllib.request import Request, urlopen

RUNTIMES = {
    "claude": "@anthropic-ai/claude-agent-sdk",
    "codex": "@openai/codex",
}


def version(value):
    if not isinstance(value, str) or not re.fullmatch(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)", value):
        raise ValueError("expected an exact stable version")
    return tuple(map(int, value.split(".")))


def fetch(url):
    request = Request(url, headers={"User-Agent": "hennery-pins-watch"})
    with urlopen(request, timeout=20) as response:
        return json.load(response)


def discover(root, get=fetch):
    pins = tomllib.loads((root / "adapters/pins.toml").read_text())
    target = {"node": pins["node"]["version"], "adapters": {}}
    changed = False
    for agent, runtime in RUNTIMES.items():
        pin = pins["adapters"][agent]
        lock = json.loads((root / f"adapters/{agent}/package-lock.json").read_text())
        current_runtime = lock["packages"][f"node_modules/{runtime}"]["version"]
        versions = {}
        for key, package, current in [("adapter", pin["package"], pin["version"]),
                                      ("runtime", runtime, current_runtime)]:
            doc = get(f"https://registry.npmjs.org/{package}/latest")
            if doc["name"] != package:
                raise ValueError("registry returned a different package")
            latest = doc["version"]
            # A prerelease or malformed tag cannot become a pin or branch name.
            try:
                newer = version(latest) > version(current)
            except ValueError:
                newer = False
            versions[key] = latest if newer else current
            changed |= newer
        target["adapters"][agent] = versions
    releases = get("https://nodejs.org/dist/index.json")
    major = version(target["node"])[0]
    stable = []
    for release in releases:
        candidate = release["version"].removeprefix("v")
        try:
            if version(candidate)[0] == major:
                stable.append((candidate, release.get("lts", False)))
        except ValueError:
            continue
    candidates = [v for v, lts in stable if lts] or [v for v, _ in stable]
    if not candidates:
        raise ValueError("Node returned no stable release for the pinned major")
    latest = max(candidates, key=version)
    if version(latest) > version(target["node"]):
        target["node"] = latest
        changed = True
    if not changed:
        return []
    # The branch identifies the entire proposed set, independent of hourly cutoff.
    digest = hashlib.sha256(json.dumps(target, sort_keys=True).encode()).hexdigest()[:16]
    return [{"pkg": "runtime", "branch": f"pins/runtime-{digest}", "pins": target,
             "npm_before": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")}]


def apply(root, update):
    path = root / "adapters/pins.toml"
    text = path.read_text()
    pins = tomllib.loads(text)
    target = update["pins"]
    if set(target) != {"node", "adapters"} or set(target["adapters"]) != set(RUNTIMES):
        raise ValueError("unexpected pin update")
    cutoff = update["npm_before"]
    # Reject invalid input before writing anything; never move the cutoff backwards.
    if (not re.fullmatch(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z", cutoff)
            or datetime.fromisoformat(cutoff) < datetime.fromisoformat(pins["npm_before"])):
        raise ValueError("invalid or older npm cutoff")
    edits = [("node", "version", target["node"], pins["node"]["version"])]
    for agent, runtime in RUNTIMES.items():
        values = target["adapters"][agent]
        if set(values) != {"adapter", "runtime"}:
            raise ValueError("unexpected adapter update")
        lock = json.loads((root / f"adapters/{agent}/package-lock.json").read_text())
        current = lock["packages"][f"node_modules/{runtime}"]["version"]
        edits.extend([(f"adapters.{agent}", "version", values["adapter"], pins["adapters"][agent]["version"]),
                      (f"adapters.{agent}.overrides", f'"{runtime}"', values["runtime"], current)])
    edits.append(("codex_app_server", "codex_version", target["adapters"]["codex"]["runtime"],
                  pins["codex_app_server"]["codex_version"]))
    for section, key, value, current in edits:
        if version(value) < version(current):
            raise ValueError("pin update would downgrade a package")
        header = f"[{section}]"
        if header not in text:
            # Only the new runtime override sections may be introduced.
            if not section.endswith(".overrides"):
                raise ValueError(f"missing section {section}")
            text += f'\n{header}\n{key} = "{value}"\n'
            continue
        pattern = rf'(?m)(^\[{re.escape(section)}\]\n(?:(?!\[)[^\n]*\n)*?^{re.escape(key)} = ")[^"]+"'
        text, count = re.subn(pattern, lambda match: match[1] + value + '"', text, count=1)
        if count != 1:
            raise ValueError(f"missing pin {section}.{key}")
    text, count = re.subn(r'(?m)^npm_before = "[^"]+"', f'npm_before = "{cutoff}"', text, count=1)
    if count != 1:
        raise ValueError("missing npm cutoff")
    tomllib.loads(text)
    path.write_text(text)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["discover", "apply"])
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[1])
    args = parser.parse_args()
    if args.command == "apply":
        apply(args.root, json.loads(os.environ["PIN_UPDATE"]))
    else:
        print(json.dumps(discover(args.root), separators=(",", ":")))


if __name__ == "__main__":
    main()
