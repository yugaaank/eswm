#!/usr/bin/env python3
"""One-off repair for specs/ YAML rot.

Two defects accumulated in hand-written spec files:

  * unquoted colons in single-line `title:` / `description:` values, which make
    the whole file invalid YAML (e.g. `title: Panics: dock underflow`).
  * status vocabulary drift: tasks say `completed` or `planned`, stories say
    `passing`, while the linter and newer files use `pending` / `passing` /
    `failing` for tasks and `pending` / `in_progress` / `completed` for stories.

Idempotent: running it twice changes nothing. Kept next to spec-lint.py so the
same repair can be applied after a bulk edit, and removed from the hot path
once the corpus is clean.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SPECS = ROOT / "specs"

TASK_DONE = {"completed"}
TASK_PENDING = {"planned"}
STORY_DONE = {"passing"}
STORY_PENDING = {"planned"}

KEY = re.compile(r"^(\s*(?:[A-Za-z_][\w-]*|-\s+[A-Za-z_][\w-]*):\s+)(.+?)(\s*)$")
STATUS = re.compile(r"^(\s*)status:\s*(\S+)\s*$")


def quote_line(line: str) -> str:
    if line.lstrip().startswith("#"):
        return line
    m = KEY.match(line)
    if not m:
        return line
    prefix, value, trail = m.groups()
    if not value or value[0] in "\"'|>[{&*!":
        return line
    if ": " not in value or value.startswith(("http://", "https://")):
        return line
    escaped = value.replace("\\", "\\\\").replace('"', '\\"')
    return f'{prefix}"{escaped}"\n'


def normalize_status(line: str, is_task_file: bool) -> str:
    m = STATUS.match(line)
    if not m:
        return line
    indent, value = m.groups()
    level = len(indent)
    if not is_task_file:
        # epic.yaml / release-plan.yaml keep story and epic vocabulary even
        # though it is indented the same way as a task status.
        if value in STORY_DONE:
            value = "completed"
        elif value in STORY_PENDING:
            value = "pending"
        return f"{indent}status: {value}\n"
    if level == 0 and value in STORY_DONE:
        value = "completed"
    elif level == 0 and value in STORY_PENDING:
        value = "pending"
    elif level > 0 and value in TASK_DONE:
        value = "passing"
    elif level > 0 and value in TASK_PENDING:
        value = "pending"
    return f"{indent}status: {value}\n"


def repair(path: Path) -> bool:
    is_task_file = path.name.endswith("-tasks.yaml")
    lines = path.read_text().splitlines(keepends=True)
    out: list[str] = []
    block_indent: int | None = None
    changed = False
    for line in lines:
        stripped = line.lstrip()
        indent = len(line) - len(stripped)
        if block_indent is not None:
            if stripped and indent <= block_indent:
                block_indent = None
            else:
                out.append(line)
                continue
        if stripped.endswith("|\n") or stripped.rstrip().endswith(("|", ">")):
            block_indent = indent
            out.append(line)
            continue
        new = normalize_status(line, is_task_file)
        new = quote_line(new)
        if new != line:
            changed = True
        out.append(new)
    if changed:
        path.write_text("".join(out))
    return changed


def main() -> int:
    changed = 0
    for path in sorted(SPECS.rglob("*.yaml")):
        if repair(path):
            print("repaired", path.relative_to(ROOT))
            changed += 1
    print(f"{changed} file(s) repaired")
    return 0


if __name__ == "__main__":
    sys.exit(main())
