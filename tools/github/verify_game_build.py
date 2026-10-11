"""Run the private native-build gate on Windows or Linux without local wrappers.

Only verification reports leave the runner. The archive stays outside the
checkout; tpfre independently verifies every archive file and native target.
"""

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys


def select_archive(selected, release_archive, updated_archive, repo):
    choices = {
        "tf3_build40408_steam_windows": release_archive,
        "tf3_build40420_steam_windows": updated_archive,
    }
    if selected not in choices:
        raise ValueError(f"No private archive mapping for native bundle {selected}")
    value = choices[selected]
    if not value:
        raise ValueError(f"No private archive configured for native bundle {selected}")
    archive = Path(value).resolve(strict=True)
    if not archive.is_dir() or archive.is_relative_to(repo.resolve()):
        raise ValueError("Private archive must be a directory outside the checkout")
    return archive


def check_report(report, commit, selected):
    if (
        report.get("source_commit") != commit
        or report.get("source_dirty") is not False
        or report.get("review_required") is not False
        or report.get("runtime_verified") is not False
        or report.get("kind") != "selected_build_verification"
        or report.get("bundle") != selected
        or not report.get("targets")
    ):
        raise ValueError("Verification must describe this exact clean commit, bundle and static targets")


def verify(repo, cache, release_archive, updated_archive, expected_commit=None, candidate=None):
    repo = repo.resolve(strict=True)
    selected = (repo / "profiles/native-build.txt").read_text(encoding="utf-8").strip()
    archive = select_archive(selected, release_archive, updated_archive, repo)
    cache = cache.resolve()
    if cache.is_relative_to(repo):
        raise ValueError("Verification build cache must stay outside the checkout")
    commit = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=repo, text=True).strip()
    if expected_commit and expected_commit != commit:
        raise ValueError("Checkout does not match the workflow commit")
    output = repo / "target/game-build-verification.json"
    output.parent.mkdir(parents=True, exist_ok=True)
    output.unlink(missing_ok=True)
    subprocess.run(
        ["cargo", "build", "--release", "--locked", "--manifest-path", "tools/tpfre/Cargo.toml",
         "--target-dir", str(cache), "--jobs", "2"], cwd=repo, check=True,
    )
    tool = cache / "release" / ("tpfre.exe" if os.name == "nt" else "tpfre")
    result = subprocess.run(
        [str(tool), "verify-build", "--repo", str(repo), "--archive", str(archive), "--json"],
        cwd=repo, text=True, stdout=subprocess.PIPE, check=False,
    )
    if result.stdout:
        output.write_text(result.stdout, encoding="utf-8")
    if result.returncode:
        raise ValueError("Selected game build verification failed; inspect the static report")
    check_report(json.loads(result.stdout), commit, selected)
    if candidate:
        candidate_path = Path(candidate).resolve(strict=True)
        if not candidate_path.is_dir() or candidate_path.is_relative_to(repo):
            raise ValueError("Candidate archive must be a directory outside the checkout")
        candidate_output = repo / "target/game-candidate-verification.json"
        candidate_output.unlink(missing_ok=True)
        result = subprocess.run(
            [str(tool), "verify", str(candidate_path), "--profiles", str(repo / "profiles"), "--json"],
            cwd=repo, text=True, stdout=subprocess.PIPE, check=False,
        )
        if result.stdout:
            candidate_output.write_text(result.stdout, encoding="utf-8")
        if result.returncode:
            raise ValueError("Candidate signatures failed; native bundle remains unchanged")
        report = json.loads(result.stdout)
        if (report.get("kind") != "profile_verification" or not report.get("targets")
                or report.get("review_required") is not False or report.get("runtime_verified") is not False):
            raise ValueError("Candidate report must contain successful static target checks only")
    print(f"Static verification passed for {selected} at {commit}; runtime remains unverified")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, default=Path.cwd())
    parser.add_argument("--cache", type=Path, required=True)
    parser.add_argument("--candidate")
    args = parser.parse_args()
    try:
        verify(args.repo, args.cache, os.environ.get("TPF3MP_GAME_ARCHIVE"),
               os.environ.get("TPF3MP_UPDATED_GAME_ARCHIVE"), os.environ.get("GITHUB_SHA"), args.candidate)
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        print(f"Game-build gate refused: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
