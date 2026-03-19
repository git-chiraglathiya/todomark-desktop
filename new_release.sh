#!/usr/bin/env bash

set -euo pipefail

if ! command -v git >/dev/null 2>&1; then
  echo "Error: git is not installed."
  exit 1
fi

if ! command -v node >/dev/null 2>&1; then
  echo "Error: node is not installed."
  exit 1
fi

if ! git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "Error: run this script inside the git repository."
  exit 1
fi

REPO_ROOT="$(git rev-parse --show-toplevel)"
cd "$REPO_ROOT"

CURRENT_BRANCH="$(git rev-parse --abbrev-ref HEAD)"
if [[ "$CURRENT_BRANCH" != "main" ]]; then
  echo "Error: current branch is '$CURRENT_BRANCH'. Please switch to 'main' before releasing."
  exit 1
fi

if ! git remote get-url origin >/dev/null 2>&1; then
  echo "Error: git remote 'origin' not found."
  exit 1
fi

CURRENT_VERSION="$(node -e "const fs=require('fs');const p=JSON.parse(fs.readFileSync('package.json','utf8'));process.stdout.write(String(p.version||''));")"
if [[ ! "$CURRENT_VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "Error: package.json version '$CURRENT_VERSION' is not valid semver (x.y.z)."
  exit 1
fi

IFS='.' read -r MAJOR MINOR PATCH <<<"$CURRENT_VERSION"

echo "Current version: $CURRENT_VERSION"
echo "Select release type:"
echo "1) patch"
echo "2) minor"
echo "3) major"

while true; do
  read -r -p "Choice [1-3]: " VERSION_CHOICE
  case "$VERSION_CHOICE" in
    1)
      PATCH=$((PATCH + 1))
      break
      ;;
    2)
      MINOR=$((MINOR + 1))
      PATCH=0
      break
      ;;
    3)
      MAJOR=$((MAJOR + 1))
      MINOR=0
      PATCH=0
      break
      ;;
    *)
      echo "Invalid choice. Enter 1, 2, or 3."
      ;;
  esac
done

NEW_VERSION="${MAJOR}.${MINOR}.${PATCH}"
NEW_TAG="v${NEW_VERSION}"

read -r -p "Commit message: " COMMIT_MESSAGE
if [[ -z "${COMMIT_MESSAGE// }" ]]; then
  echo "Error: commit message cannot be empty."
  exit 1
fi

if git rev-parse "$NEW_TAG" >/dev/null 2>&1; then
  echo "Error: git tag '$NEW_TAG' already exists locally."
  exit 1
fi

if git ls-remote --exit-code --tags origin "refs/tags/${NEW_TAG}" >/dev/null 2>&1; then
  echo "Error: git tag '$NEW_TAG' already exists on origin."
  exit 1
fi

echo "Bumping version to $NEW_VERSION ..."
NEW_VERSION="$NEW_VERSION" node <<'NODE'
const fs = require("fs");

const version = process.env.NEW_VERSION;
const files = ["package.json", "src-tauri/tauri.conf.json", "package-lock.json"];

for (const file of files) {
  if (!fs.existsSync(file)) continue;
  const json = JSON.parse(fs.readFileSync(file, "utf8"));
  json.version = version;
  if (file === "package-lock.json" && json.packages && json.packages[""]) {
    json.packages[""].version = version;
  }
  fs.writeFileSync(file, `${JSON.stringify(json, null, 2)}\n`);
}
NODE

echo "Running release git flow..."
git add .

if git diff --cached --quiet; then
  echo "Error: nothing to commit after version update."
  exit 1
fi

git commit -m "$COMMIT_MESSAGE"
git tag -a "$NEW_TAG" -m "$NEW_TAG"
git push origin main
git push origin "$NEW_TAG"

echo "Release done."
echo "Version: $NEW_VERSION"
echo "Tag: $NEW_TAG"
