#!/usr/bin/env bash
# Apply the project's branch-protection policy to `master`.
#
# Requires the GitHub CLI (`gh`) authenticated with repo-admin rights.
# This is the one-time setup run by a repo admin after the scaffold PR
# merges. It is idempotent — re-running applies the same policy.
set -euo pipefail

REPO="${REPO:-vedanthvdev/dayseam}"
BRANCH="${BRANCH:-master}"

echo "Applying branch protection to ${REPO}#${BRANCH}"

gh api -X PUT "repos/${REPO}/branches/${BRANCH}/protection" \
  -H "Accept: application/vnd.github+json" \
  -f "required_status_checks[strict]=true" \
  -f "required_status_checks[contexts][]=rust" \
  -f "required_status_checks[contexts][]=frontend" \
  -f "required_status_checks[contexts][]=check-semver-label" \
  -F "enforce_admins=true" \
  -F "required_pull_request_reviews[required_approving_review_count]=1" \
  -F "required_pull_request_reviews[dismiss_stale_reviews]=true" \
  -F "required_pull_request_reviews[require_code_owner_reviews]=false" \
  -F "restrictions=" \
  -F "required_linear_history=true" \
  -F "allow_force_pushes=false" \
  -F "allow_deletions=false" \
  -F "required_conversation_resolution=true" \
  -F "lock_branch=false" \
  -F "allow_fork_syncing=false"

echo "Enabling auto-delete of head branches after merge"
gh api -X PATCH "repos/${REPO}" -F "delete_branch_on_merge=true" >/dev/null

echo "Setting merge options (squash only)"
gh api -X PATCH "repos/${REPO}" \
  -F "allow_merge_commit=false" \
  -F "allow_rebase_merge=false" \
  -F "allow_squash_merge=true" >/dev/null

echo "Branch protection applied."
