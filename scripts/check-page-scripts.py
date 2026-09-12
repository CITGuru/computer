#!/usr/bin/env python3
"""Parse the scripts this crate evaluates inside a page.

They are Rust string constants, so nothing compiles them: a typo in one reaches
a caller as a failed `find` against a real browser, which is the one place
neither a test nor clippy looks.
"""

import re
import subprocess
import sys

SOURCE = "crates/computer-core/src/cdp.rs"


def constants(src):
    for name in ("MATCH", "SELECTOR", "DESCRIBE"):
        found = re.search(r"const " + name + r': &str = r#"(.*?)"#;', src, re.S)
        if not found:
            sys.exit(f"{name} is not in {SOURCE} under the name this expects")
        yield name, found.group(1)


def main():
    src = open(SOURCE).read()
    scripts = dict(constants(src))

    # What `describe()` does, and the reason it is worth checking: the splice is
    # a string replacement that a compiler never sees.
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

    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
