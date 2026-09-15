#!/usr/bin/env python3
"""Parse the page scripts and the accessibility reader, which nothing compiles."""

import re
import subprocess
import sys

SOURCE = "crates/computer-core/src/cdp.rs"
READER = "crates/computer-core/images/desktop/a11y.py"


def constants(src):
    for name in ("MATCH", "SELECTOR", "DESCRIBE"):
        found = re.search(r"const " + name + r': &str = r#"(.*?)"#;', src, re.S)
        if not found:
            sys.exit(f"{name} is not in {SOURCE} under the name this expects")
        yield name, found.group(1)


def main():
    src = open(SOURCE).read()
    scripts = dict(constants(src))

    # `describe()` splices SELECTOR in by string replacement, which no compiler sees.
    scripts["DESCRIBE"] = scripts["DESCRIBE"].replace("SELECTOR_FN", scripts["SELECTOR"])

    failed = False
    for name, body in scripts.items():
        probe = f"const f = {body};\nif (typeof f !== 'function') throw new Error('not a function');\n"
        ran = subprocess.run(
            ["node", "--input-type=module", "-e", probe],
            capture_output=True,
            text=True,
        )

        print(f"{name:9} {'ok' if ran.returncode == 0 else 'FAILED'}")
        if ran.returncode != 0:
            print(ran.stderr.strip()[:800], file=sys.stderr)
            failed = True

    # Compiled, not run: it connects to a bus on import.
    reader = subprocess.run(
        [sys.executable, "-c", f"compile(open({READER!r}).read(), {READER!r}, 'exec')"],
        capture_output=True,
        text=True,
    )
    print(f"{'a11y.py':9} {'ok' if reader.returncode == 0 else 'FAILED'}")
    if reader.returncode != 0:
        print(reader.stderr.strip()[:800], file=sys.stderr)
        failed = True

    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
