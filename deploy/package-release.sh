#!/usr/bin/env bash
set -euo pipefail

project_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
crate_version=$(sed -nE 's/^version = "([^"]+)"/\1/p' "${project_dir}/crates/stats-cli/Cargo.toml" | head -n 1)
version=${STATS_VERSION:-${crate_version}}
version=${version#v}
if [[ -z ${version} ]]; then
    echo "could not determine package version" >&2
    exit 1
fi
if [[ ${version} != "${crate_version}" ]]; then
    echo "release version ${version} does not match crate version ${crate_version}" >&2
    exit 1
fi

platform=${STATS_PLATFORM:-linux-x86_64}
target=${STATS_TARGET:-}
case "${STATS_PLUGIN_SUFFIX:-${platform}}" in
    *.so|so|*linux*) plugin_suffix=so ;;
    *.dylib|dylib|*macos*|*darwin*) plugin_suffix=dylib ;;
    *.dll|dll|*windows*) plugin_suffix=dll ;;
    *) plugin_suffix=${STATS_PLUGIN_SUFFIX:-so} ;;
esac
output_dir=${STATS_RELEASE_DIR:-${project_dir}/target/release-artifacts}
if command -v sha256sum >/dev/null 2>&1; then
    checksum() { sha256sum "$@"; }
else
    checksum() { shasum -a 256 "$@"; }
fi
staging_dir=$(mktemp -d)
trap 'rm -rf "${staging_dir}"' EXIT

cd "${project_dir}"
if [[ -n ${target} ]]; then
    cargo build --workspace --release --locked --target "${target}"
    release_dir="${project_dir}/target/${target}/release"
else
    cargo build --workspace --release --locked
    release_dir="${project_dir}/target/release"
fi

core_name="stats-${version}-${platform}"
http_name="stats-http-${version}-${platform}"
cgi_name="stats-cgi-${version}-${platform}"
plugins_name="stats-plugins-${version}-${platform}"

install_worker() {
    install -m 0755 "${release_dir}/stats-report-worker" "$1/stats-report-worker"
}

core_dir="${staging_dir}/${core_name}"
install -d "${core_dir}/deploy"
install -m 0755 "${release_dir}/stats-cli" "${core_dir}/stats"
install_worker "${core_dir}"
install -m 0755 deploy/install.sh "${core_dir}/deploy/"
install -m 0644 deploy/stats-update.service deploy/stats-update.timer deploy/stats-logrotate "${core_dir}/deploy/"
install -m 0644 examples/awstats.example.test.conf README.md CHANGELOG.md "${core_dir}/"

http_dir="${staging_dir}/${http_name}"
install -d "${http_dir}"
install -m 0755 "${release_dir}/stats-http" "${http_dir}/stats-http"
install_worker "${http_dir}"

cgi_dir="${staging_dir}/${cgi_name}"
install -d "${cgi_dir}"
install -m 0755 "${release_dir}/stats-cgi" "${cgi_dir}/stats-cgi"
install_worker "${cgi_dir}"

plugins_dir="${staging_dir}/${plugins_name}"
install -d "${plugins_dir}"
for plugin in html json csv; do
    library=$(find "${release_dir}" -maxdepth 1 -type f -name "libstats_report_${plugin}.${plugin_suffix}" -print -quit)
    if [[ -z ${library} ]]; then
        echo "could not find report_${plugin} plugin library" >&2
        exit 1
    fi
    install -m 0755 "${library}" "${plugins_dir}/$(basename "${library}")"
done

mkdir -p "${output_dir}"
for bundle_dir in "${core_dir}" "${http_dir}" "${cgi_dir}" "${plugins_dir}"; do
    bundle_name=$(basename "${bundle_dir}")
    archive="${output_dir}/${bundle_name}.tar.gz"
    tar -C "${staging_dir}" -czf "${archive}" "${bundle_name}"
    # Keep the checksum file relocatable: CI, release uploads, and users verify
    # it from the directory containing the archive rather than from this
    # checkout's absolute path.
    (cd "${output_dir}" && checksum "$(basename "${archive}")") >"${archive}.sha256"
    echo "created ${archive}"
    echo "created ${archive}.sha256"
done
