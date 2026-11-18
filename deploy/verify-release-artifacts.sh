#!/usr/bin/env bash
set -euo pipefail

artifact_dir=${1:?usage: verify-release-artifacts.sh <artifact-dir>}
if command -v sha256sum >/dev/null 2>&1; then
    checksum_check() { sha256sum -c "$@"; }
else
    checksum_check() { shasum -a 256 -c "$@"; }
fi

archive_for() {
    local prefix=$1
    local pattern="${prefix}-*.tar.gz"
    if [[ ${prefix} == stats ]]; then
        pattern='stats-[0-9]*.tar.gz'
    fi
    find "${artifact_dir}" -maxdepth 1 -type f -name "${pattern}" -print -quit
}

assert_entry() {
    local listing=$1
    local pattern=$2
    local description=$3
    if ! grep -Eq "${pattern}" <<<"${listing}"; then
        echo "release archive is missing ${description}" >&2
        return 1
    fi
}

verify_archive() {
    local prefix=$1
    local archive
    archive=$(archive_for "${prefix}")
    [[ -n ${archive} ]] || { echo "missing ${prefix} archive" >&2; return 1; }
    (cd "${artifact_dir}" && checksum_check "$(basename "${archive}").sha256")
    tar -tzf "${archive}"
}

core_listing=$(verify_archive stats)
assert_entry "${core_listing}" '/stats$' 'core stats binary'
assert_entry "${core_listing}" '/stats-report-worker$' 'core report worker'
assert_entry "${core_listing}" '/deploy/install\.sh$' 'installer'

http_listing=$(verify_archive stats-http)
assert_entry "${http_listing}" '/stats-http$' 'HTTP binary'
assert_entry "${http_listing}" '/stats-report-worker$' 'HTTP report worker'

cgi_listing=$(verify_archive stats-cgi)
assert_entry "${cgi_listing}" '/stats-cgi$' 'CGI binary'
assert_entry "${cgi_listing}" '/stats-report-worker$' 'CGI report worker'

plugins_listing=$(verify_archive stats-plugins)
for plugin in html json csv; do
    assert_entry "${plugins_listing}" "/libstats_report_${plugin}\\.(so|dylib|dll)$" "report_${plugin} plugin"
done

echo "release archives verified in ${artifact_dir}"
