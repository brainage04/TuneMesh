#!/bin/sh
# Checks what a release tag needs in the tree: the metainfo <release> of the
# version and the newest rpm %changelog entry carry the tag's date, and
# docs/release-notes/<tag>.md exists (the GitHub Release's notes).
#
#   packaging/ci/check-release.sh v0.1.0 [YYYY-MM-DD]
#
# The date defaults to the tag's own date: the tagger date of an annotated
# tag, the commit date of a lightweight one, in the time zone it was made in.
set -eu

tag=${1:?usage: check-release.sh TAG [YYYY-MM-DD]}
version=${tag#v}
cd "$(dirname "$0")/../.."
date=${2:-$(git for-each-ref --format='%(creatordate:short)' "refs/tags/$tag")}
if [ -z "$date" ]; then
    echo "::error::no tag $tag in this checkout"
    exit 1
fi
fail=0

metainfo=linux/data/io.github.brainage04.Spotless.metainfo.xml
v_re=$(printf '%s\n' "$version" | sed 's/\./\\./g')
got=$(sed -n "s/.*<release version=\"$v_re\" date=\"\([0-9-]*\)\".*/\1/p" "$metainfo")
if [ "$got" != "$date" ]; then
    echo "::error file=$metainfo::<release version=\"$version\"> has date \"$got\", the tag $tag is from $date"
    fail=1
fi

spec=packaging/rpm/spotless.spec
entry=$(sed -n '/^%changelog/,$ { /^\* /{ p; q; } }' "$spec")
# "* Mon Oct 19 2026 Name <mail> - 0.1.0-1": the date is fields 3-5.
entry_date=$(echo "$entry" | awk '{ print $3, $4, $5 }')
entry_date=$(LC_ALL=C date -d "$entry_date" +%F 2>/dev/null || true)
case $entry in
    *" - $version-"*) entry_version=$version ;;
    *) entry_version= ;;
esac
if [ "$entry_date" != "$date" ] || [ "$entry_version" != "$version" ]; then
    echo "::error file=$spec::the newest %changelog entry is \"$entry\"; it must be for $version, dated $date"
    fail=1
fi

notes=docs/release-notes/$tag.md
if [ ! -s "$notes" ]; then
    echo "::error::$notes is missing or empty; it holds the release notes"
    fail=1
fi

exit "$fail"
