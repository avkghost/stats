#!/usr/bin/env bash
set -euo pipefail

project_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
version=${STATS_VERSION:-0.1.0}
platform=${STATS_PLATFORM:-linux-x86_64}
output_dir=${STATS_RELEASE_DIR:-${project_dir}/target/release-artifacts}
bundle_name="stats-${version}-${platform}"
staging_dir=$(mktemp -d)
trap 'rm -rf "${staging_dir}"' EXIT

cd "${project_dir}"
cargo build --workspace --release
cargo package --workspace --allow-dirty --no-verify

bundle_dir="${staging_dir}/${bundle_name}"
install -d "${bundle_dir}/deploy"
install -m 0755 target/release/stats-cli "${bundle_dir}/stats"
install -m 0755 deploy/install.sh "${bundle_dir}/deploy/"
install -m 0644 deploy/stats-update.service deploy/stats-update.timer deploy/stats-logrotate "${bundle_dir}/deploy/"
install -m 0644 README.md CHANGELOG.md "${bundle_dir}/"

mkdir -p "${output_dir}"
tar -C "${staging_dir}" -czf "${output_dir}/${bundle_name}.tar.gz" "${bundle_name}"
sha256sum "${output_dir}/${bundle_name}.tar.gz" >"${output_dir}/${bundle_name}.tar.gz.sha256"
echo "created ${output_dir}/${bundle_name}.tar.gz"
echo "created ${output_dir}/${bundle_name}.tar.gz.sha256"
