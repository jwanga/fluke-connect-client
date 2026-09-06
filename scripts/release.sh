#!/usr/bin/env bash
# Publish the crate version named by a release tag to crates.io.
#
#   scripts/release.sh v0.3.0
#
# Run by .github/workflows/release.yml when a `v*` tag is pushed; also runs
# locally with `cargo login` credentials or CARGO_REGISTRY_TOKEN set. It does
# not bump versions, write the changelog or create the GitHub Release: the
# local release procedure does those before pushing the tag (see
# docs/DESIGN.md, "Public repository hygiene"). This script only checks that
# the tag is the checked-out commit, names its version and sits on main,
# then publishes.
set -euo pipefail

usage() {
    echo "usage: $0 v<major>.<minor>.<patch>[-<pre>]" >&2
    exit 2
}

tag=${1:-}
[[ -n "$tag" ]] || usage
if [[ ! "$tag" =~ ^v([0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?)$ ]]; then
    echo "error: '$tag' is not a v<semver> release tag" >&2
    usage
fi
version=${BASH_REMATCH[1]}

cd "$(git rev-parse --show-toplevel)"

tag_commit=$(git rev-parse --verify --quiet "refs/tags/$tag^{commit}") ||
    { echo "error: tag $tag does not exist locally" >&2; exit 1; }
if [[ "$tag_commit" != "$(git rev-parse HEAD)" ]]; then
    echo "error: HEAD $(git rev-parse --short HEAD) is not the commit tagged $tag" >&2
    exit 1
fi

read -r name manifest_version < <(cargo metadata --no-deps --format-version 1 |
    python3 -c '
import json, sys
packages = json.load(sys.stdin)["packages"]
assert len(packages) == 1, f"expected one package, found {len(packages)}"
print(packages[0]["name"], packages[0]["version"])')

if [[ "$manifest_version" != "$version" ]]; then
    echo "error: tag $tag names $version but Cargo.toml says $manifest_version" >&2
    echo "hint: bump [package] version (and Cargo.lock) before tagging" >&2
    exit 1
fi

# A tag pushed from a feature branch must not publish the feature branch.
git fetch --quiet origin main
if ! git merge-base --is-ancestor HEAD origin/main; then
    echo "error: the tagged commit $(git rev-parse --short HEAD) is not on main" >&2
    exit 1
fi

# Re-running the workflow after a transient failure must not fail on the
# publish that already happened.
status=$(curl --silent --output /dev/null --write-out '%{http_code}' \
    --header "User-Agent: $name release script (https://github.com/jwanga/fluke-connect-client)" \
    "https://crates.io/api/v1/crates/$name/$version")
case "$status" in
    200)
        echo "$name $version is already on crates.io; nothing to do"
        exit 0
        ;;
    404) ;;
    *)
        echo "error: crates.io returned HTTP $status for $name $version" >&2
        exit 1
        ;;
esac

echo "publishing $name $version"
cargo publish --locked
