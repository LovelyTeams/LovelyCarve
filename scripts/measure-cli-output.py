"""Measure stdout characters on a reproducible System32-style folder fixture.

python scripts/measure-cli-output.py <new-cli> --baseline <old-cli>
No real evidence or host-specific paths are written into the repository.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("executable", type=Path)
    parser.add_argument("--baseline", type=Path)
    options = parser.parse_args()
    environment = dict(os.environ, CLAUDECODE="1")
    for key in ("LOVELYCARVE_IMAGE", "LOVELYCARVE_CASE", "LOVELYCARVE_SESSION", "RUST_LOG"):
        environment.pop(key, None)
    with tempfile.TemporaryDirectory(prefix="lovelycarve-output-") as directory:
        evidence = Path(directory) / "System32"
        evidence.mkdir()
        for index in range(600):
            path = evidence / f"system-library-{index:04}.dll"
            path.write_bytes(b"fixture\n")
            os.utime(path, (1719792000, 1719792000))

        def measure(executable, label, extra=()):
            result = subprocess.run([str(executable.resolve()), "-q", "--timezone", "UTC",
                                     "--image", str(evidence), "files", "list", "--path", "/System32", *extra],
                                    env=environment, capture_output=True, text=True, encoding="utf-8",
                                    timeout=120, check=True)
            data = json.loads(result.stdout)
            return {"mode": label, "items": len(data["items"]), "stdoutChars": len(result.stdout),
                    "stderrChars": len(result.stderr), "hasNextCursor": bool(data["nextCursor"])}

        results = []
        if options.baseline:
            results.append(measure(options.baseline, "before-default"))
        fields = ["--fields", "name,id,kind,size,timestamps.modified"]
        results.extend([
            measure(options.executable, "after-500", ["--page-size", "500"]),
            measure(options.executable, "after-500-projected", ["--page-size", "500", *fields]),
            measure(options.executable, "after-agent-default"),
            measure(options.executable, "after-agent-projected", fields),
        ])
        print(json.dumps(results, indent=2))


if __name__ == "__main__":
    main()
