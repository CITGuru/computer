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
    drop: tuple = ()
    argless: tuple = ()
    run_as: int | None = None
    because: dict = field(default_factory=dict)


ARGS = {
    "--packages": ("EXTRA_PACKAGES", "packages"),
    "--sources": ("EXTRA_SOURCES", "sources"),
    "--apps": ("EXTRA_APPS", "apps"),
}


RULES = {
    "e2b": Rules(
        drop=("LABEL", "CMD", "USER"),
        argless=("EXTRA_PACKAGES", "EXTRA_SOURCES", "EXTRA_APPS"),
        run_as=1000,
        because={
            "LABEL": 'rejected: "Unsupported instruction: LABEL"',
            "CMD": "ignored; the start command comes from --cmd, which is required",
            "USER": "overridden: E2B appends its own, so the image's would only "
            "stop the lines after it from running as root",
            "EXTRA_PACKAGES, EXTRA_SOURCES, EXTRA_APPS": "no --build-arg, and an ARG "
            'default keeps its quotes, so apt is asked for a package named "" four '
            "minutes into the build. --packages, --sources and --apps write the values "
            "into files the build reads; without them the lines that use them go",
            "run_as": "E2B appends `USER user` (uid 1000), which cannot write a HOME "
            "that WORKDIR created for root, so chromium never makes its profile",
        },
    ),
}


def home_of(dockerfile: str) -> str:
    for pattern in (r"^ENV\s+HOME=(\S+)", r"^WORKDIR\s+(\S+)"):
        found = re.search(pattern, dockerfile, re.MULTILINE)
        if found:
            return found.group(1)
    return "/root"


def rewrite(dockerfile: str, rules: Rules, carried: dict | None = None) -> str:
    carried = carried or {}
    kept, dropping = [], False

    for line in dockerfile.splitlines(keepends=True):
        stripped = line.strip()

        if dropping:
            dropping = stripped.endswith("\\")
            continue

        instruction = stripped.split(" ", 1)[0].upper()
        argless = any(name in stripped for name in rules.argless)

        if instruction in rules.drop:
            dropping = stripped.endswith("\\")
            continue
        if instruction == "ARG" and argless:
            continue
        if instruction == "RUN" and argless:
            if any(name in stripped and carried.get(name) for name in rules.argless):
                kept.append(line)
                continue
            dropping = stripped.endswith("\\")
            continue

        kept.append(line)

    if rules.run_as is not None:
        home = home_of(dockerfile)
        kept.append(
            f"\n# {rules.because.get('run_as', 'the vendor runs as another user')}\n"
            f"RUN mkdir -p {home} && chown -R {rules.run_as}:{rules.run_as} {home}\n"
        )

    return carrying("".join(kept), carried)


def carrying(dockerfile: str, carried: dict) -> str:
    if not carried:
        return dockerfile

    for name in carried:
        at = kept_at(name)
        dockerfile = dockerfile.replace(f'[ -n "${name}" ]', f'[ -s {at} ]')
        dockerfile = dockerfile.replace(f'"${name}"', f'"$(cat {at})"')
        dockerfile = dockerfile.replace(f"${name}", f"$(cat {at})")

    lines = dockerfile.splitlines(keepends=True)
    for at, line in enumerate(lines):
        if "/usr/local/share/computer-" in line:
            lines.insert(at, "COPY computer-* /usr/local/share/\n")
            break

    return "".join(lines)


def kept_at(name: str) -> str:
    return f"/usr/local/share/computer-{name.removeprefix('EXTRA_').lower()}"


def values(argv: list[str]) -> dict:
    carried = {}

    for flag, (name, _) in ARGS.items():
        if flag in argv:
            given = argv[argv.index(flag) + 1]
            if given.strip():
                carried[name] = given
    return carried


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

    carried = values(argv)
    for name, given in carried.items():
        at = target / f"computer-{name.removeprefix('EXTRA_').lower()}"
        at.write_text(given if given.endswith("\n") else given + "\n")

    dockerfile = target / "Dockerfile"
    dockerfile.write_text(rewrite(dockerfile.read_text(), rules, carried))

    said = ", ".join(sorted(carried)) or "nothing to carry in"
    print(f"context: {target}  ({vendor}: {', '.join(rules.because)}; {said})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
