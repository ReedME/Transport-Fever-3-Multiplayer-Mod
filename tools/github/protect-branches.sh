#!/usr/bin/env bash
# Makes GitHub enforce the branch rules in AGENTS.md, for everyone,
# administrators included:
#
# - dev, acceptance and main can be neither force-pushed nor deleted;
# - dev takes pull requests only, once their ci checks passed, and one
#   that changes the decisions or the plan only with the owner's approval
#   (.github/CODEOWNERS). The owner, an administrator, may still push to
#   it, as promotions and fixes need;
# - acceptance takes only commits whose ci checks passed;
# - main takes only commits whose ci and acceptance checks passed.
#
# Promotions are fast-forwards of commits already tested on the branch
# before, so they pass; a commit made on the branch itself, such as a pull
# request's merge commit, has no checks yet and is refused.
#
# Run it once as a repository administrator, with the GitHub CLI signed in:
#
#   tools/github/protect-branches.sh [owner/repo]
#
# Running it again reapplies the same rules. Check names are the jobs of
# .github/workflows/ci.yml and acceptance.yml; update both lists when a job
# is renamed or added.
set -euo pipefail

repo="${1:-ReedME/Transport-Fever-3-Multiplayer-Mod}"
# GitHub Actions, so only checks from this repository's workflows count.
actions_app=15368

ci=(
  "test (windows-latest)" "test (ubuntu-latest)" "test (macos-latest)"
  "release build (windows-latest)" "release build (ubuntu-latest)"
  "release build (macos-latest)" "server image" "launcher page" "tpfre"
  "release-day tools"
)
acceptance=(
  "load (windows-latest)" "load (ubuntu-latest)" "load (macos-latest)"
  "soak (ubuntu-latest)"
)

# dev's protection body: pull requests only, their ci checks passed, a code
# owner's approval for the files .github/CODEOWNERS names; administrators
# not bound, so the owner can promote and fix.
dev_rules() {
  local checks="" name
  for name in "${ci[@]}"; do
    checks+="${checks:+,}{\"context\":\"$name\",\"app_id\":$actions_app}"
  done
  printf '{"required_status_checks":{"strict":false,"checks":[%s]},' "$checks"
  printf '"enforce_admins":false,'
  printf '"required_pull_request_reviews":{"dismiss_stale_reviews":true,'
  printf '"require_code_owner_reviews":true,"required_approving_review_count":0},'
  printf '"restrictions":null,"allow_force_pushes":false,"allow_deletions":false,'
  printf '"required_linear_history":false}'
}

# The protection body for a branch that requires the checks named after it.
rules() {
  local checks="" name
  for name in "$@"; do
    checks+="${checks:+,}{\"context\":\"$name\",\"app_id\":$actions_app}"
  done
  local required="null"
  if [ -n "$checks" ]; then
    required="{\"strict\":false,\"checks\":[$checks]}"
  fi
  printf '{"required_status_checks":%s,"enforce_admins":true,' "$required"
  printf '"required_pull_request_reviews":null,"restrictions":null,'
  printf '"allow_force_pushes":false,"allow_deletions":false,'
  printf '"required_linear_history":false}'
}

protect() {
  local branch="$1"
  shift
  rules "$@" | gh api -X PUT "repos/$repo/branches/$branch/protection" --input - \
    --jq '"'"$branch"': \(.required_status_checks.checks // [] | length) required checks, force-push \(.allow_force_pushes.enabled), deletion \(.allow_deletions.enabled), admins bound \(.enforce_admins.enabled)"'
}

dev_rules | gh api -X PUT "repos/$repo/branches/dev/protection" --input - \
  --jq '"dev: pull requests only, \(.required_status_checks.checks | length) required checks, code owners \(.required_pull_request_reviews.require_code_owner_reviews), admins bound \(.enforce_admins.enabled)"'
protect acceptance "${ci[@]}"
protect main "${ci[@]}" "${acceptance[@]}"
