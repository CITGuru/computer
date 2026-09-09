"""Derive a builder context from an image directory.

    python3 images/context.py images/desktop /tmp/ctx --for e2b
    python3 images/context.py --list

Some sandbox vendors build a subset of the Dockerfile language, so an image a
container runtime accepts does not go over unchanged. Each vendor's differences
are one entry in RULES; the transform itself knows nothing about any of them.

 `LABEL computer.profile` is what a machine reads to refuse a
mismatched box, and everything in a container runs as root.
"""

import re
import shutil
import sys
from dataclasses import dataclass, field
from pathlib import Path


@dataclass(frozen=True)
class Rules:
    """What one vendor's builder cannot take."""

    #: Instructions dropped entirely, with their line continuations.
    drop: tuple = ()
    #: Build arguments to drop, along with the RUN lines that read them.
    drop_arg: tuple = ()
    #: The uid the vendor runs as, where it is not the one the image builds as.
    run_as: int | None = None
    #: Why each of the above, for whoever reads the output and wonders.
    because: dict = field(default_factory=dict)


RULES = {
    "e2b": Rules(
        drop=("LABEL", "CMD", "USER"),
        drop_arg=("EXTRA_PACKAGES",),
        run_as=1000,
        because={
            "LABEL": 'rejected: "Unsupported instruction: LABEL"',
            "CMD": "ignored; the start command comes from --cmd, which is required",
            "USER": "overridden: E2B appends its own, so the image's would only "
            "stop the lines after it from running as root",
            "EXTRA_PACKAGES": 'an ARG default keeps its quotes, so apt is asked for a '
            'package named "" four minutes into the build',
            "run_as": "E2B appends `USER user` (uid 1000), which cannot write a HOME "
            "that WORKDIR created for root, so chromium never makes its profile",
        },
    ),
}


def home_of(dockerfile: str) -> str:
    """The HOME the image gives its user, from the image rather than a guess."""
    for pattern in (r"^ENV\s+HOME=(\S+)", r"^WORKDIR\s+(\S+)"):
        found = re.search(pattern, dockerfile, re.MULTILINE)
        if found:
            return found.group(1)
    return "/root"


def rewrite(dockerfile: str, rules: Rules) -> str:
    """The same Dockerfile, with what this vendor cannot take taken out."""
    kept, dropping = [], False

    for line in dockerfile.splitlines(keepends=True):
        stripped = line.strip()

        if dropping:
            dropping = stripped.endswith("\\")
            continue

        instruction = stripped.split(" ", 1)[0].upper()
        drops_arg = any(name in stripped for name in rules.drop_arg)

        if instruction in rules.drop:
            dropping = stripped.endswith("\\")
            continue
        if instruction == "ARG" and drops_arg:
            continue
        if instruction == "RUN" and drops_arg:
            dropping = stripped.endswith("\\")
            continue

        kept.append(line)

    if rules.run_as is not None:
        home = home_of(dockerfile)
        kept.append(
            f"\n# {rules.because.get('run_as', 'the vendor runs as another user')}\n"
            f"RUN mkdir -p {home} && chown -R {rules.run_as}:{rules.run_as} {home}\n"
        )

    return "".join(kept)


def main(argv: list[str]) -> int:
    if "--list" in argv:
        for name, rules in RULES.items():
            print(f"{name}:")
            for what, why in rules.because.items():
                print(f"  {what:<16} {why}")
        return 0

    if len(argv) < 3:
        print(__doc__.strip(), file=sys.stderr)
        return 2

    source, target = Path(argv[1]), Path(argv[2])
    vendor = argv[argv.index("--for") + 1] if "--for" in argv else "e2b"

    rules = RULES.get(vendor)
    if rules is None:
        known = ", ".join(sorted(RULES))
        print(f"no rules for {vendor}; known: {known}", file=sys.stderr)
        return 2

    shutil.rmtree(target, ignore_errors=True)
    target.mkdir(parents=True)
    for entry in source.iterdir():
        if entry.is_file():
            shutil.copy2(entry, target / entry.name)

    dockerfile = target / "Dockerfile"
    dockerfile.write_text(rewrite(dockerfile.read_text(), rules))

    print(f"context: {target}  ({vendor}: {', '.join(rules.because)})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
