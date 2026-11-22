"""Measure stdout characters of directory pages on a reproducible System32-style folder fixture.

python scripts/measure-cli-output.py <cli> [--baseline <other cli build>]
No real evidence or host-specific paths are written into the repository.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile

MODES = [
    ("text-200", []),
    ("json-200", ["--json"]),
    ("json-500", ["--json", "--limit", "500"]),
    ("json-500-projected", ["--json=name,path,kind,size,timestamps.modified", "--limit", "500"]),
    ("json-50-projected", ["--json=name,path,kind,size,timestamps.modified", "--limit", "50"]),
]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("executable", type=Path)
    parser.add_argument("--baseline", type=Path)
    options = parser.parse_args()
    environment = dict(os.environ)
    for key in ("LOVELYCARVE_CASE", "LOVELYCARVE_BACKEND_DIR", "RUST_LOG"):
        environment.pop(key, None)
    with tempfile.TemporaryDirectory(prefix="lovelycarve-output-") as directory:
        evidence = Path(directory) / "System32"
        evidence.mkdir()
        for index in range(600):
            path = evidence / f"system-library-{index:04}.dll"
            path.write_bytes(b"fixture\n")
            os.utime(path, (1719792000, 1719792000))

        def measure(executable, build, mode, extra):
            words = [str(executable.resolve()), "-q", "--tz", "UTC", "--image", str(evidence), *extra[:1],
                     "ls", "/System32/folder:", *extra[1:]]
            result = subprocess.run(words, env=environment, capture_output=True, text=True, encoding="utf-8",
                                    timeout=120, check=True, cwd=directory)
            entries = (len(json.loads(result.stdout)["items"]) if extra and extra[0].startswith("--json")
                       else max(len(result.stdout.splitlines()) - 2, 0))
            return {"build": build, "mode": mode, "entries": entries,
                    "stdoutChars": len(result.stdout), "stderrChars": len(result.stderr)}

        results = []
        for build, executable in [("baseline", options.baseline), ("current", options.executable)]:
            if executable is None:
                continue
            results.extend(measure(executable, build, mode, extra) for mode, extra in MODES)
        print(json.dumps(results, indent=2))


if __name__ == "__main__":
    main()
