#!/usr/bin/env python3

import argparse
import functools
import re
import subprocess
from pathlib import Path


TAG_RE = re.compile(r"^v(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?$")
COMMIT_RE = re.compile(
    r"^(?P<kind>[A-Za-z]+)(?:\([^)]*\))?(?:!)?:\s*(?P<body>.+)$"
)

CATEGORIES = {
    "feat": "Features",
    "fix": "Bug Fixes",
    "refactor": "Refactor",
    "perf": "Performance",
}
SKIPPED_KINDS = {"release", "ci", "chore", "docs", "build", "style", "test", "fmt"}


def git(repo, *args):
    return subprocess.check_output(
        ["git", *args],
        cwd=repo,
        text=True,
        stderr=subprocess.STDOUT,
    ).strip()


def parse_version(tag):
    match = TAG_RE.match(tag)
    if not match:
        return None

    major, minor, patch, prerelease = match.groups()
    core = (int(major), int(minor), int(patch))
    pre = None if prerelease is None else tuple(prerelease.split("."))
    return core, pre


def compare_identifier(left, right):
    left_numeric = left.isdigit()
    right_numeric = right.isdigit()

    if left_numeric and right_numeric:
        return (int(left) > int(right)) - (int(left) < int(right))
    if left_numeric:
        return -1
    if right_numeric:
        return 1
    return (left > right) - (left < right)


def compare_versions(left, right):
    left_core, left_pre = left
    right_core, right_pre = right

    if left_core != right_core:
        return (left_core > right_core) - (left_core < right_core)
    if left_pre is None and right_pre is None:
        return 0
    if left_pre is None:
        return 1
    if right_pre is None:
        return -1

    for left_part, right_part in zip(left_pre, right_pre):
        result = compare_identifier(left_part, right_part)
        if result != 0:
            return result
    return (len(left_pre) > len(right_pre)) - (len(left_pre) < len(right_pre))


def find_previous_tag(repo, tag):
    current = parse_version(tag)
    if current is None:
        raise ValueError(f"release tag must look like v0.7.0 or v0.7.0-pre.1: {tag}")

    tags = []
    for candidate in git(repo, "tag", "--list", "v[0-9]*").splitlines():
        version = parse_version(candidate)
        if version is not None and compare_versions(version, current) < 0:
            tags.append((candidate, version))

    if not tags:
        return None

    tags.sort(
        key=functools.cmp_to_key(
            lambda left, right: compare_versions(left[1], right[1])
        )
    )
    return tags[-1][0]


def commit_subjects(repo, previous_tag, tag):
    if previous_tag is None:
        return [git(repo, "log", "-1", "--format=%B", tag)]

    # Ignore patch-equivalent commits when a release line was rebased
    output = git(
        repo,
        "log",
        "--cherry-pick",
        "--right-only",
        "--pretty=format:%s",
        f"{previous_tag}...{tag}",
    )
    return output.splitlines() if output else []


def normalized_subject(subject):
    subject = " ".join(subject.strip().split())
    if not subject:
        return None, None

    match = COMMIT_RE.match(subject)
    if match:
        kind = match.group("kind").lower()
        body = match.group("body").strip()
        if kind in CATEGORIES:
            return CATEGORIES[kind], body
        if kind in SKIPPED_KINDS:
            return None, None

    lowered = subject.lower()
    if lowered.startswith("release ") or TAG_RE.match(subject):
        return None, None
    if lowered.startswith("update readme") or lowered.startswith("docs "):
        return None, None

    return "Other Changes", subject


def grouped_subjects(subjects):
    groups = {title: [] for title in [*CATEGORIES.values(), "Other Changes"]}
    seen = set()

    for subject in subjects:
        category, body = normalized_subject(subject)
        if category is None:
            continue

        key = (category, body.casefold())
        if key in seen:
            continue

        seen.add(key)
        groups[category].append(body)

    return groups


def render_notes(repo, tag, repository):
    git(repo, "rev-parse", "--verify", f"{tag}^{{commit}}")

    previous_tag = find_previous_tag(repo, tag)
    if previous_tag is None:
        return git(repo, "log", "-1", "--format=%B", tag) + "\n"

    groups = grouped_subjects(commit_subjects(repo, previous_tag, tag))
    lines = ["## What's Changed", ""]

    wrote_section = False
    for title in [*CATEGORIES.values(), "Other Changes"]:
        subjects = groups[title]
        if not subjects:
            continue

        wrote_section = True
        lines.append(f"### {title}")
        lines.extend(f"- {subject}" for subject in subjects)
        lines.append("")

    if not wrote_section:
        lines.append("No user-facing changes in this release.")
        lines.append("")

    lines.append(
        f"**Full Changelog**: https://github.com/{repository}/compare/{previous_tag}...{tag}"
    )
    lines.append("")
    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--tag", required=True)
    parser.add_argument("--repo", type=Path, default=Path("."))
    parser.add_argument("--repository", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()

    args.output.write_text(
        render_notes(args.repo, args.tag, args.repository),
        encoding="utf-8",
    )


if __name__ == "__main__":
    main()
