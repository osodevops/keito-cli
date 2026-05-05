#!/usr/bin/env python3
"""Validate the package version used by release CI workflows."""

from __future__ import annotations

import argparse
import os
import re
import subprocess
import sys
import tomllib
from pathlib import Path
from typing import Any


def fail(message: str) -> None:
    print(f"::error::{message}", file=sys.stderr)
    raise SystemExit(1)


def parse_toml(text: str, source: str) -> dict[str, Any]:
    try:
        return tomllib.loads(text)
    except tomllib.TOMLDecodeError as exc:
        fail(f"Could not parse {source}: {exc}")


def load_package_version(text: str, source: str) -> str:
    manifest = parse_toml(text, source)

    try:
        version = manifest["package"]["version"]
    except KeyError:
        try:
            version = manifest["workspace"]["package"]["version"]
        except KeyError as exc:
            fail(f"Could not read package version from {source}: {exc}")

    if not isinstance(version, str):
        fail(f"Package version in {source} must be a string.")

    return version


def load_head_version() -> str:
    return load_package_version(Path("Cargo.toml").read_text(), "Cargo.toml")


def load_base_version(base_ref: str) -> str:
    try:
        text = subprocess.check_output(
            ["git", "show", f"{base_ref}:Cargo.toml"],
            text=True,
            stderr=subprocess.PIPE,
        )
    except subprocess.CalledProcessError as exc:
        fail(f"Could not read Cargo.toml at {base_ref}: {exc.stderr.strip()}")

    return load_package_version(text, f"{base_ref}:Cargo.toml")


def load_workspace_package_names() -> set[str]:
    manifest_path = Path("Cargo.toml")
    manifest = parse_toml(manifest_path.read_text(), str(manifest_path))

    names: set[str] = set()
    package = manifest.get("package")
    if isinstance(package, dict) and isinstance(package.get("name"), str):
        names.add(package["name"])

    workspace = manifest.get("workspace")
    if isinstance(workspace, dict):
        for member in workspace.get("members", []):
            member_manifest_path = Path(member) / "Cargo.toml"
            try:
                member_manifest = parse_toml(
                    member_manifest_path.read_text(), str(member_manifest_path)
                )
                member_package = member_manifest["package"]
                member_name = member_package["name"]
            except (OSError, KeyError, TypeError) as exc:
                fail(f"Could not read package name from {member_manifest_path}: {exc}")

            if not isinstance(member_name, str):
                fail(f"Package name in {member_manifest_path} must be a string.")
            names.add(member_name)

    if not names:
        fail("No package names found in Cargo.toml.")

    return names


def parse_semver(version: str) -> tuple[int, int, int, str | None]:
    match = re.fullmatch(
        r"(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)"
        r"(?:-([0-9A-Za-z.-]+))?(?:\+[0-9A-Za-z.-]+)?",
        version,
    )
    if not match:
        fail(f"Invalid Cargo SemVer version: {version}")

    major, minor, patch, prerelease = match.groups()
    return int(major), int(minor), int(patch), prerelease


def compare_prerelease(left: str | None, right: str | None) -> int:
    if left == right:
        return 0
    if left is None:
        return 1
    if right is None:
        return -1

    left_parts = left.split(".")
    right_parts = right.split(".")
    for left_part, right_part in zip(left_parts, right_parts):
        if left_part == right_part:
            continue

        left_numeric = left_part.isdigit()
        right_numeric = right_part.isdigit()
        if left_numeric and right_numeric:
            return 1 if int(left_part) > int(right_part) else -1
        if left_numeric:
            return -1
        if right_numeric:
            return 1
        return 1 if left_part > right_part else -1

    if len(left_parts) == len(right_parts):
        return 0
    return 1 if len(left_parts) > len(right_parts) else -1


def compare_semver(left: str, right: str) -> int:
    left_major, left_minor, left_patch, left_pre = parse_semver(left)
    right_major, right_minor, right_patch, right_pre = parse_semver(right)

    left_core = (left_major, left_minor, left_patch)
    right_core = (right_major, right_minor, right_patch)
    if left_core != right_core:
        return 1 if left_core > right_core else -1

    return compare_prerelease(left_pre, right_pre)


def verify_lockfile_version(version: str) -> None:
    try:
        lockfile = tomllib.loads(Path("Cargo.lock").read_text())
    except (OSError, tomllib.TOMLDecodeError) as exc:
        fail(f"Could not read Cargo.lock: {exc}")

    package_names = load_workspace_package_names()
    mismatches: list[str] = []
    for package in lockfile.get("package", []):
        name = package.get("name")
        package_version = package.get("version")
        if name in package_names and package_version != version:
            mismatches.append(f"{name} is {package_version}")

    if mismatches:
        fail(
            "Cargo.lock is not in sync with Cargo.toml version "
            f"{version}: {', '.join(mismatches)}"
        )


def verify_changelog_version(version: str) -> None:
    try:
        changelog = Path("CHANGELOG.md").read_text()
    except OSError as exc:
        fail(f"Could not read CHANGELOG.md: {exc}")

    heading = re.compile(rf"^## \[{re.escape(version)}\]", re.MULTILINE)
    if not heading.search(changelog):
        fail(f"CHANGELOG.md is missing a ## [{version}] release section.")


def tag_exists(tag: str) -> bool:
    return (
        subprocess.run(
            ["git", "rev-parse", "--verify", "--quiet", f"refs/tags/{tag}"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
        ).returncode
        == 0
    )


def write_env(path: str | None, values: dict[str, str]) -> None:
    if not path:
        return

    with open(path, "a", encoding="utf-8") as env_file:
        for key, value in values.items():
            env_file.write(f"{key}={value}\n")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--base-ref", required=True)
    parser.add_argument("--mode", choices=("guard", "tagger"), required=True)
    parser.add_argument("--event-name", default=os.environ.get("GITHUB_EVENT_NAME", ""))
    parser.add_argument("--github-env", default=os.environ.get("GITHUB_ENV"))
    args = parser.parse_args()

    base_version = load_base_version(args.base_ref)
    head_version = load_head_version()
    version_changed = head_version != base_version
    tag = f"v{head_version}"

    write_env(
        args.github_env,
        {
            "BASE_VERSION": base_version,
            "HEAD_VERSION": head_version,
            "RELEASE_TAG": tag,
            "VERSION_CHANGED": "true" if version_changed else "false",
        },
    )

    if args.mode == "tagger" and not version_changed:
        print(f"Package version is still {head_version}; no release tag required.")
        return

    if compare_semver(head_version, base_version) <= 0:
        fail(
            "Release-impacting changes require Cargo.toml version to increase "
            f"(base: {base_version}, head: {head_version})."
        )

    verify_lockfile_version(head_version)
    verify_changelog_version(head_version)

    if args.mode == "guard" and args.event_name == "pull_request" and tag_exists(tag):
        fail(f"Release tag {tag} already exists. Bump Cargo.toml to a fresh version.")

    print(f"Release version check passed: {base_version} -> {head_version} ({tag}).")


if __name__ == "__main__":
    main()
