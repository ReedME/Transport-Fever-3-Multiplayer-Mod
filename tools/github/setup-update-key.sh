#!/usr/bin/env bash
# Configure protected Ed25519 updates without drafting or publishing a release.
# The PEM stays outside Git and reaches gh only through stdin.
set -euo pipefail
repo="${1:-ReedME/Transport-Fever-3-Multiplayer-Mod}"
key_dir="${2:?Usage: setup-update-key.sh [owner/repo] /private/key/directory}"
existing=$(gh variable list --repo "$repo" --json name --jq '[.[] | select(.name == "TPF3MP_UPDATE_PUBLIC_KEY")] | length')
if [ "$existing" != 0 ]; then
  echo "Refusing to replace configured update trust; signing-key rotation needs a separate explicit procedure." >&2
  exit 1
fi
mkdir -p "$key_dir"
key_dir=$(cd "$key_dir" && pwd -P)
if git -C "$key_dir" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "The private key directory must stay outside every Git checkout." >&2
  exit 1
fi
umask 077
chmod 700 "$key_dir"
key="$key_dir/update-signing-key.pem"
if [ -e "$key" ]; then
  echo "Refusing to replace an existing signing key; use a fresh directory for rotation." >&2
  exit 1
fi
openssl genpkey -algorithm ed25519 -out "$key"
chmod 600 "$key"
public=$(openssl pkey -in "$key" -pubout -outform DER | tail -c 32 | base64 | tr -d '\n')
owner_id=$(gh api user --jq .id)
printf '{"reviewers":[{"type":"User","id":%s}],"prevent_self_review":false,"deployment_branch_policy":{"protected_branches":false,"custom_branch_policies":true}}' "$owner_id" \
  | gh api -X PUT "repos/$repo/environments/release" --input - >/dev/null
policies=$(gh api "repos/$repo/environments/release/deployment-branch-policies" --jq '[.branch_policies[] | select(.name == "v*" and .type == "tag")] | length')
if [ "$policies" = 0 ]; then
  gh api -X POST "repos/$repo/environments/release/deployment-branch-policies" \
    -f 'name=v*' -f 'type=tag' >/dev/null
fi
gh secret set TPF3MP_UPDATE_SIGNING_KEY --env release --repo "$repo" < "$key"
gh variable set TPF3MP_UPDATE_PUBLIC_KEY --repo "$repo" --body "$public"
printf 'Protected signing configured for %s. No release was dispatched.\n' "$repo"
printf 'Private backup: %s (0600, outside Git). Keep it in secure offline storage.\n' "$key"
