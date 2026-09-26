#!/bin/sh
# bumps maw to a version and tags it; pushing the tag lets the release workflow finish the template
# usage: tools/release.sh <version>, then git push --follow-tags
set -eu

# 0.3.0 or v0.3.0
version=${1#v}

# only the version bump goes into the release commit, and only on a tree that passes
if ! git diff --quiet HEAD; then
	echo "error: uncommitted changes; commit or stash them first" >&2
	exit 1
fi
cargo test -q >/dev/null
cargo clippy -q --all-targets -- -D warnings

# the package's version in Cargo.toml, its entry in Cargo.lock, and the one nix configs see
sed -i "0,/^version = .*/s//version = \"$version\"/" Cargo.toml
sed -i "/^name = \"maw\"$/{n;s/^version = .*/version = \"$version\"/}" Cargo.lock
sed -i "s/^  mawVersion = .*/  mawVersion = \"$version\";/" nix/lib.nix

# the man pages carry the version too
MAW_BLESS=1 cargo test -q --test generated >/dev/null

git add man
git commit -qm "maw $version" Cargo.toml Cargo.lock nix/lib.nix man
git tag -a "v$version" -m "maw $version"
echo "tagged v$version; git push --follow-tags to release"
