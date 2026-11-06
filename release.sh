#!/bin/bash

set -euo pipefail

SCRIPT_DIR="$(cd -P -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
cd "${SCRIPT_DIR}"

if [ -n "$(git status --porcelain)" ]; then
    echo 'Error: There are uncommitted changes in your working directory'
    echo 'Please commit or discard the changes before proceeding'
    exit 1
fi

cargo format
cargo lint
cargo t --all-features
cargo b --all-features

pnpx changelogen@latest --bump --hideAuthorEmail
new_version=$(node -p "require('./package.json').version")
cargo set-version "$new_version"
git checkout -- ./CHANGELOG.md ./package.json
git add ./Cargo.toml
pnpx changelogen@latest --hideAuthorEmail --push --release
cargo publish
