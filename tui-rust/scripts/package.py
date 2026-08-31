#!/usr/bin/env python3
"""Create a release archive with project and Rust dependency license notices."""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import tarfile
import zipfile
from pathlib import Path


NOTICE_PREFIXES = ("license", "copying", "notice", "unlicense")
TOOLCHAIN_NOTICE_PREFIXES = NOTICE_PREFIXES + ("copyright",)
PINNED_RUST_VERSION = "1.98.0"


def run(command: list[str], cwd: Path) -> str:
    completed = subprocess.run(
        command,
        cwd=cwd,
        check=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    return completed.stdout.strip()


def safe_name(value: str) -> str:
    return re.sub(r"[^A-Za-z0-9._+-]", "-", value)


def dependency_packages(metadata: dict, root_id: str) -> list[dict]:
    packages = {package["id"]: package for package in metadata["packages"]}
    nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
    reached: set[str] = set()
    pending = [root_id]

    while pending:
        package_id = pending.pop()
        if package_id in reached:
            continue
        reached.add(package_id)
        node = nodes.get(package_id)
        if node is None:
            continue
        for dependency in node["deps"]:
            kinds = dependency.get("dep_kinds") or []
            if not kinds or any(item.get("kind") != "dev" for item in kinds):
                pending.append(dependency["pkg"])

    return sorted(
        (packages[package_id] for package_id in reached if package_id != root_id),
        key=lambda package: (package["name"].lower(), package["version"]),
    )


def license_files(package: dict) -> list[Path]:
    package_dir = Path(package["manifest_path"]).resolve().parent
    candidates: list[Path] = []
    declared = package.get("license_file")
    if declared:
        declared_path = Path(declared)
        if not declared_path.is_absolute():
            declared_path = package_dir / declared_path
        if declared_path.is_file():
            candidates.append(declared_path.resolve())

    for child in package_dir.iterdir():
        if child.is_file() and child.name.lower().startswith(NOTICE_PREFIXES):
            candidates.append(child.resolve())

    return sorted(set(candidates), key=lambda path: path.name.lower())


def rust_toolchain_notices(tui_root: Path) -> tuple[list[Path], dict[str, Path]]:
    rustc_version = run(["rustc", "--version"], tui_root)
    actual_version = rustc_version.split()[1] if len(rustc_version.split()) > 1 else ""
    if actual_version != PINNED_RUST_VERSION:
        raise RuntimeError(
            f"release packaging requires rustc {PINNED_RUST_VERSION}; "
            f"found {rustc_version}"
        )

    notice_dir = tui_root / "licenses" / f"rust-{PINNED_RUST_VERSION}"
    if not notice_dir.is_dir():
        raise RuntimeError(f"missing checked-in Rust notice directory: {notice_dir}")
    notices: dict[str, Path] = {}
    for child in notice_dir.iterdir():
        if child.is_file() and child.name.lower().startswith(
            TOOLCHAIN_NOTICE_PREFIXES
        ):
            notices.setdefault(child.name.lower(), child.resolve())

    templates: dict[str, Path] = {}
    for path in notices.values():
        normalized = path.name.lower().replace("_", "-")
        if normalized.startswith("license-mit"):
            templates.setdefault("MIT", path)
        elif normalized.startswith("license-apache"):
            templates.setdefault("Apache-2.0", path)

    missing = sorted({"MIT", "Apache-2.0"} - templates.keys())
    if missing:
        raise RuntimeError(
            f"checked-in Rust {PINNED_RUST_VERSION} notices do not provide: "
            + ", ".join(missing)
        )
    return sorted(notices.values(), key=lambda path: path.name.lower()), templates


def fallback_license_files(
    package: dict, templates: dict[str, Path]
) -> list[tuple[Path, str]]:
    expression = re.sub(r"\s+", " ", package.get("license") or "").strip()
    identifiers: list[str]
    if expression == "MIT":
        identifiers = ["MIT"]
    elif expression == "Apache-2.0":
        identifiers = ["Apache-2.0"]
    elif expression in {
        "MIT OR Apache-2.0",
        "Apache-2.0 OR MIT",
        "MIT/Apache-2.0",
        "Apache-2.0/MIT",
    }:
        identifiers = ["MIT", "Apache-2.0"]
    else:
        return []
    return [
        (templates[identifier], f"SPDX-LICENSE-{identifier}.txt")
        for identifier in identifiers
    ]


def copy_toolchain_notices(notices: list[Path], destination: Path) -> list[str]:
    destination.mkdir(parents=True, exist_ok=True)
    copied: list[str] = []
    for notice in notices:
        target = destination / notice.name
        shutil.copy2(notice, target)
        copied.append(str(target.relative_to(destination.parents[1])))
    return copied


def copy_dependency_notices(
    packages: list[dict], destination: Path, templates: dict[str, Path]
) -> list[dict]:
    inventory: list[dict] = []
    missing: list[str] = []

    for package in packages:
        source = package.get("source") or ""
        if not source.startswith("registry+"):
            continue
        notices = license_files(package)
        label = f'{safe_name(package["name"])}-{safe_name(package["version"])}'
        if notices:
            notice_entries = [(notice, notice.name) for notice in notices]
            origin = "upstream-package"
        else:
            notice_entries = fallback_license_files(package, templates)
            origin = "rust-toolchain-spdx-template"
        if not notice_entries:
            missing.append(
                f'{package["name"]} {package["version"]} '
                f'(declared {package.get("license") or "UNKNOWN"})'
            )
            continue
        target = destination / label
        target.mkdir(parents=True, exist_ok=True)
        for notice, target_name in notice_entries:
            shutil.copy2(notice, target / target_name)
        inventory.append(
            {
                "name": package["name"],
                "version": package["version"],
                "license": package.get("license"),
                "authors": package.get("authors") or [],
                "source": source,
                "licenseFileOrigin": origin,
                "files": [
                    f"licenses/rust/{label}/{target_name}"
                    for _, target_name in notice_entries
                ],
            }
        )

    if missing:
        raise RuntimeError(
            "dependency packages without an upstream notice or supported SPDX fallback:\n- "
            + "\n- ".join(missing)
        )
    return inventory


def write_archive(stage: Path, output: Path) -> None:
    if output.suffix == ".zip":
        with zipfile.ZipFile(
            output,
            "w",
            compression=zipfile.ZIP_DEFLATED,
            strict_timestamps=False,
        ) as archive:
            for path in sorted(stage.rglob("*")):
                if path.is_file():
                    archive.write(path, path.relative_to(stage.parent).as_posix())
        return
    with tarfile.open(output, "w:gz") as archive:
        archive.add(stage, arcname=stage.name)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--platform", required=True)
    parser.add_argument("--target", required=True)
    args = parser.parse_args()

    tui_root = Path(__file__).resolve().parents[1]
    repo_root = tui_root.parent
    binary = args.binary.resolve()
    if not binary.is_file():
        raise SystemExit(f"release binary not found: {binary}")

    metadata = json.loads(
        run(
            [
                "cargo",
                "metadata",
                "--locked",
                "--format-version",
                "1",
                "--filter-platform",
                args.target,
            ],
            tui_root,
        )
    )
    root_id = next(
        package_id
        for package_id in metadata["workspace_members"]
        if next(package for package in metadata["packages"] if package["id"] == package_id)["name"]
        == "sshmgr-tui"
    )
    root = next(package for package in metadata["packages"] if package["id"] == root_id)
    version = root["version"]
    bundle_name = f"sshmgr-tui-{version}-{safe_name(args.platform)}"
    executable_name = f"sshmgr-tui-{safe_name(args.platform)}"
    if binary.suffix.lower() == ".exe":
        executable_name += ".exe"

    packages_root = tui_root / "dist" / "packages"
    stage = packages_root / bundle_name
    if stage.exists():
        shutil.rmtree(stage)
    stage.mkdir(parents=True)
    shutil.copy2(binary, stage / executable_name)
    for name in ("LICENSE", "THIRD_PARTY_NOTICES.md", "REBUILDING.md"):
        shutil.copy2(repo_root / name, stage / name)
    shutil.copy2(tui_root / "Cargo.lock", stage / "Cargo.lock")

    toolchain_notices, license_templates = rust_toolchain_notices(tui_root)
    toolchain_files = copy_toolchain_notices(
        toolchain_notices,
        stage / "licenses" / "rust-toolchain",
    )
    inventory = copy_dependency_notices(
        dependency_packages(metadata, root_id),
        stage / "licenses" / "rust",
        license_templates,
    )
    source_commit = os.environ.get("GITHUB_SHA") or run(
        ["git", "rev-parse", "HEAD"], repo_root
    )
    build_metadata = {
        "name": root["name"],
        "version": version,
        "platform": args.platform,
        "target": args.target,
        "sourceCommit": source_commit,
        "rustcVersion": run(["rustc", "--version"], tui_root),
        "rustToolchainLicenseFiles": toolchain_files,
        "dependencies": inventory,
    }
    (stage / "BUILD-METADATA.json").write_text(
        json.dumps(build_metadata, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )

    extension = ".zip" if args.platform.startswith("windows-") else ".tar.gz"
    output = packages_root / f"{bundle_name}{extension}"
    if output.exists():
        output.unlink()
    write_archive(stage, output)
    print(output)


if __name__ == "__main__":
    main()
