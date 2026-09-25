#!/bin/sh
# after a v<version> tag is pushed: points srcpkgs/maw/template at the tag's tarball with its checksum, committed to master
# usage: finish-release.sh <tag> [tarball-url], run from a checkout of master
set -eu

tag=$1
version=${tag#v}
url=${2:-https://github.com/AamindMandragora/maw/archive/refs/tags/$tag.tar.gz}

# the tagged commit must already carry the version, so the built binary reports it
if ! git show "$tag:Cargo.toml" | grep -qx "version = \"$version\""; then
	echo "error: Cargo.toml at $tag isn't version $version; tag with tools/release.sh" >&2
	exit 1
fi

# checksum the tarball xbps-src will fetch; github can take a moment to serve a new tag. downloaded to a file first,
# since a failed curl in a pipe would checksum nothing
tarball=$(mktemp)
curl -fsSL --retry 5 --retry-all-errors -o "$tarball" "$url"
checksum=$(sha256sum "$tarball" | cut -d' ' -f1)

# point the template at the new version
sed -i -e "s/^version=.*/version=$version/" -e "s/^revision=.*/revision=1/" -e "s/^checksum=.*/checksum=$checksum/" srcpkgs/maw/template
git commit -qm "srcpkgs/maw: update to $version" srcpkgs/maw/template
echo "maw $version: checksum $checksum"
