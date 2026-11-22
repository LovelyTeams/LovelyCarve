"""Helpers shared by the CLI end-to-end tests: one process per command, JSONL
batches, and an environment that keeps the caller's case, case backends and
quiet setting out of the cases under test."""
import json
from pathlib import Path
import subprocess

REPOSITORY = Path(__file__).resolve().parents[1]

# Variables of the caller's shell that would change what a command works on.
CALLER_SCOPE = ("LOVELYCARVE_CASE", "LOVELYCARVE_BACKEND_DIR", "LOVELYCARVE_QUIET", "RUST_LOG")


def isolated(environment, root, **extra):
    """`environment` without the caller's scope, with case backends registered
    below `root` so that no test sees another's."""
    clean = {key: value for key, value in environment.items() if key not in CALLER_SCOPE}
    clean["LOVELYCARVE_BACKEND_DIR"] = str(Path(root) / "backends")
    clean.update(extra)
    return clean


def request(identifier, *argv, output=None):
    """One batch request: the command line as argv."""
    body = {"id": identifier, "argv": [str(word) if isinstance(word, Path) else word for word in argv]}
    if output is not None:
        body["output"] = str(output)
    return body


def node(alias, node_id):
    """The address of a node by its ID."""
    return f"/{alias}/@{node_id}"


def path_id(prefix, path):
    """A path-keyed node ID: the reader's prefix and the hex of the path."""
    return prefix + path.encode().hex()


def says(stderr, text):
    """Whether stderr says `text`, in a JSON error line or a framed block that
    wrapped it."""
    def message(line):
        try:
            record = json.loads(line)
        except ValueError:
            return line
        body = (record.get("error") or record.get("warning")) if isinstance(record, dict) else None
        return body["message"] if isinstance(body, dict) else line
    flat = "".join(map(message, stderr.splitlines()))
    flat = "".join(flat.replace("│", "").replace("┌", "").replace("└", "").split())
    return "".join(text.split()) in flat


class Cli:
    """The CLI under test, run in `cwd` with `environment`."""

    def __init__(self, executable, cwd, environment):
        self.executable = Path(executable)
        self.cwd = Path(cwd)
        self.environment = environment

    def run(self, *arguments, stdin=None, expected=0, binary=False, env=None, timeout=300):
        words = [str(word) for word in arguments]
        if binary and isinstance(stdin, str):
            stdin = stdin.encode()
        options = {} if binary else {"text": True, "encoding": "utf-8"}
        result = subprocess.run([str(self.executable), *words], input=stdin, capture_output=True,
                                cwd=self.cwd, env=self.environment if env is None else env,
                                timeout=timeout, **options)
        assert result.returncode == expected, (words, result.returncode, result.stdout[:2000], result.stderr[-4000:])
        return result

    def json(self, *arguments, **options):
        """A command's result as JSON."""
        return json.loads(self.run("--json", *arguments, **options).stdout)

    def batch(self, requests, *arguments, expected=0, jobs=None, env=None):
        """Run `requests` through `batch`; returns the process and the response
        rows in the order they were written."""
        words = [*arguments, "batch", *(["--jobs", str(jobs)] if jobs else [])]
        stdin = "".join(json.dumps(item, ensure_ascii=False) + "\n" for item in requests)
        result = self.run(*words, stdin=stdin, expected=expected, env=env)
        return result, [json.loads(line) for line in result.stdout.splitlines() if line.strip()]

    def results(self, requests, *arguments, jobs=None, env=None):
        """Run `requests` that must all succeed; returns their results by ID."""
        _, rows = self.batch(requests, *arguments, jobs=jobs, env=env)
        assert all("result" in row for row in rows), rows
        answered, asked = [row["id"] for row in rows], [item["id"] for item in requests]
        # Serial batches answer in request order; parallel ones as requests finish.
        assert answered == asked if jobs is None else sorted(answered) == sorted(asked), rows
        return {row["id"]: row["result"] for row in rows}
