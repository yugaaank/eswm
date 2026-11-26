#!/usr/bin/env python3
"""Spec contract linter.

specs/ is the source of truth, so it gets the same treatment as code: parsed,
cross-referenced, and failed on drift. Run locally or in CI.

Checks
  1. every specs/**/*.yaml parses
  2. every release-plan epic has a capsule directory and an epic.yaml
  3. execution-status epics match the capsule directories
  4. every task declares id / description / verify / status
  5. task status values are known
  6. verify commands are executable for this crate: no `cargo test --lib`
     (eswm is a bin crate), no placeholder filters (e.g. `cargo test e06s02t1`)
  7. a completed task's verify filter actually appears in src/ (proof the test
     the task claims exists is really there)
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parent.parent
SPECS = ROOT / "specs"
SRC = ROOT / "src"

TASK_STATUS = {"pending", "passing", "failing", "done", "blocked"}
STORY_STATUS = {"pending", "in_progress", "completed"}
PLACEHOLDER = re.compile(r"^e\d{2}s\d{2}t\d+$")

errors: list[str] = []


def fail(msg: str) -> None:
    errors.append(msg)


def load_yaml(path: Path):
    try:
        return yaml.safe_load(path.read_text())
    except Exception as exc:  # noqa: BLE001 - report and continue
        fail(f"{path.relative_to(ROOT)}: invalid YAML: {exc}")
        return None


def check_all_yaml() -> None:
    for path in sorted(SPECS.rglob("*.yaml")):
        load_yaml(path)


def find_capsules(eid: str) -> list[Path]:
    # Completed epics retire to specs/epics/archive/ instead of vanishing.
    # release-plan is a historical index, so archived capsules still count —
    # e08 didn't disappear, it just moved to the retirement home.
    return [
        p
        for base in (SPECS / "epics", SPECS / "epics" / "archive")
        for p in base.glob(f"{eid}-*")
        if p.is_dir()
    ]


def check_release_plan() -> list[str]:
    plan = load_yaml(SPECS / "release-plan.yaml") or {}
    epics = plan.get("epics") or []
    ids = []
    for epic in epics:
        eid = epic.get("id")
        ids.append(eid)
        capsules = find_capsules(eid)
        if not capsules:
            fail(f"release-plan: epic {eid} has no specs/epics/{eid}-* capsule")
            continue
        if not (capsules[0] / "epic.yaml").exists():
            fail(f"release-plan: {capsules[0].name} has no epic.yaml")
        if epic.get("status") not in STORY_STATUS | {"planned"}:
            fail(f"release-plan: epic {eid} has unknown status {epic.get('status')!r}")
    return ids


def check_execution_status(epic_ids: list[str]) -> None:
    data = load_yaml(SPECS / "execution-status.yaml") or {}
    capsules = {
        p.name
        for base in (SPECS / "epics", SPECS / "epics" / "archive")
        for p in base.glob("e*-*")
        if p.is_dir()
    }
    missing = capsules - set(data)
    extra = set(data) - capsules
    for key in sorted(missing):
        fail(f"execution-status: epic {key} exists but is not listed")
    for key in sorted(extra):
        fail(f"execution-status: lists {key} with no capsule directory")
    for key, value in data.items():
        if value not in STORY_STATUS:
            fail(f"execution-status: {key} has unknown status {value!r}")


def src_text() -> str:
    return "\n".join(p.read_text() for p in sorted(SRC.glob("*.rs")))


def check_tasks() -> None:
    source = src_text()
    for path in sorted(SPECS.glob("epics/*/*-tasks.yaml")):
        data = load_yaml(path) or {}
        rel = path.relative_to(ROOT)
        for task in data.get("tasks") or []:
            tid = task.get("id", "<missing id>")
            for field in ("id", "description", "verify", "status"):
                if not task.get(field):
                    fail(f"{rel}: task {tid} is missing `{field}`")
            status = task.get("status")
            if status not in TASK_STATUS:
                fail(f"{rel}: task {tid} has unknown status {status!r}")
            verify = str(task.get("verify") or "")
            if "--lib" in verify:
                fail(
                    f"{rel}: task {tid} verifies with `--lib`, but eswm is a bin "
                    f"crate; use `cargo test --bin eswm <filter>`"
                )
            if verify.startswith("cargo test"):
                parts = verify.split()
                if parts[:2] != ["cargo", "test"]:
                    continue
                filt = next(
                    (p for p in parts[2:] if not p.startswith("-")), ""
                )
                if PLACEHOLDER.match(filt):
                    fail(
                        f"{rel}: task {tid} verifies with placeholder filter "
                        f"`{filt}`; name a real test"
                    )
                elif status in {"passing", "done"} and filt and filt not in source:
                    fail(
                        f"{rel}: task {tid} is {status} but its verify filter "
                        f"`{filt}` matches no test in src/"
                    )


def check_story_headers() -> None:
    for path in sorted(SPECS.glob("epics/*/*-tasks.yaml")):
        data = load_yaml(path) or {}
        rel = path.relative_to(ROOT)
        if data.get("status") not in STORY_STATUS:
            fail(f"{rel}: story has unknown status {data.get('status')!r}")
        if not data.get("tasks"):
            fail(f"{rel}: story has no tasks")


def main() -> int:
    check_all_yaml()
    epic_ids = check_release_plan()
    check_execution_status(epic_ids)
    check_tasks()
    check_story_headers()
    if errors:
        print(f"spec-lint: {len(errors)} problem(s)")
        for err in errors:
            print(f"  - {err}")
        return 1
    print(
        f"spec-lint: ok ({len(list(SPECS.glob('epics/*/*-tasks.yaml')))} task files, "
        f"{len(epic_ids)} epics)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
