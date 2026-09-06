"""Check all locked crate release dates before cargo downloads/builds crates."""
import concurrent.futures
import datetime
import json
from pathlib import Path
import tomllib
import urllib.request

HERE = Path(__file__).resolve().parent
CUTOFF = datetime.datetime(2026, 9, 4, 2, 4, 36, tzinfo=datetime.timezone.utc)
packages = tomllib.loads((HERE / "rust/Cargo.lock").read_text())["package"]

def check(package):
    name, version = package["name"], package["version"]
    url = f"https://crates.io/api/v1/crates/{name}/{version}"
    request = urllib.request.Request(url, headers={"User-Agent": "Quarry review-engine spike"})
    with urllib.request.urlopen(request, timeout=45) as response:
        metadata = json.load(response)["version"]
    published = metadata["created_at"]
    date = datetime.datetime.fromisoformat(published.replace("Z", "+00:00"))
    return {"name": name, "version": version, "published": published,
            "eligible": date < CUTOFF}

with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
    checked = list(pool.map(check, [p for p in packages if "source" in p]))
(HERE / "crate-age-audit.json").write_text(json.dumps(
    {"cutoff": CUTOFF.isoformat(), "packages": checked}, indent=2) + "\n")
failed = [p for p in checked if not p["eligible"]]
print(json.dumps({"checked": len(checked), "too_recent": failed}))
if failed:
    raise SystemExit(1)
