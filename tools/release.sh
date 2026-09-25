#!/bin/sh
# bumps maw to a version and tags it; pushing the tag lets the release workflow finish the template
# usage: tools/release.sh <version>, then git push --follow-tags
set -eu

version=$1

# the package's version in Cargo.toml, its entry in Cargo.lock, and the one nix configs see
sed -i "0,/^version = .*/s//version = \"$version\"/" Cargo.toml
sed -i "/^name = \"maw\"$/{n;s/^version = .*/version = \"$version\"/}" Cargo.lock
sed -i "s/^  mawVersion = .*/  mawVersion = \"$version\";/" nix/lib.nix

git commit -qm "maw $version" Cargo.toml Cargo.lock nix/lib.nix
git tag -a "v$version" -m "maw $version"
echo "tagged v$version; git push --follow-tags to release"
