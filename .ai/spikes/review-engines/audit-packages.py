"""Check every resolved npm package before npm ci installs anything."""
import concurrent.futures
import datetime
import json
from pathlib import Path
import urllib.parse
import urllib.request

HERE = Path(__file__).resolve().parent
CUTOFF = datetime.datetime(2026, 9, 4, 2, 4, 36, tzinfo=datetime.timezone.utc)
lock = json.loads((HERE / "package-lock.json").read_text())
packages = {}
for path, info in lock["packages"].items():
    if not path or not info.get("version"):
        continue
    name = info.get("name") or path.rsplit("node_modules/", 1)[-1]
    packages[(name, info["version"])] = info

def check(key):
    name, version = key
    url = "https://registry.npmjs.org/" + urllib.parse.quote(name, safe="")
    with urllib.request.urlopen(url, timeout=30) as response:
        metadata = json.load(response)
    published = metadata["time"][version]
    date = datetime.datetime.fromisoformat(published.replace("Z", "+00:00"))
    return {"name": name, "version": version, "published": published,
            "eligible": date < CUTOFF}

with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
    checked = list(pool.map(check, sorted(packages)))
report = {"cutoff": CUTOFF.isoformat(), "packages": checked}
(HERE / "package-age-audit.json").write_text(json.dumps(report, indent=2) + "\n")
failed = [entry for entry in checked if not entry["eligible"]]
print(json.dumps({"checked": len(checked), "too_recent": failed}))
if failed:
    raise SystemExit(1)
