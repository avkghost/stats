#!/usr/bin/env bash
set -euo pipefail

bundle=${1:?usage: smoke-test-installer.sh <core-bundle.tar.gz|extracted-bundle>}
tmp_dir=$(mktemp -d)
trap 'rm -rf "${tmp_dir}"' EXIT

if [[ -f ${bundle} ]]; then
    tar -C "${tmp_dir}" -xzf "${bundle}"
    bundle_dir=$(find "${tmp_dir}" -mindepth 1 -maxdepth 1 -type d -print -quit)
else
    bundle_dir=$(cd "${bundle}" && pwd)
fi

[[ -n ${bundle_dir} && -x ${bundle_dir}/stats && -x ${bundle_dir}/stats-report-worker ]]
[[ -x ${bundle_dir}/deploy/install.sh ]]

mock_bin=${tmp_dir}/mock-bin
install_root=${tmp_dir}/install
mkdir -p "${mock_bin}" "${install_root}"
cat >"${mock_bin}/systemctl" <<'MOCK'
#!/usr/bin/env bash
set -euo pipefail
exit 0
MOCK
chmod 0755 "${mock_bin}/systemctl"

# The installer requires root, but all paths are redirected into a disposable
# prefix. Reuse the invoking root account/group to avoid creating host users.
STATS_USER=$(id -un) \
STATS_GROUP=$(id -gn) \
STATS_BINARY_PATH="${install_root}/bin/stats" \
STATS_CONFIG_PATH="${install_root}/etc/stats.conf" \
STATS_DATA_DIR="${install_root}/var/lib" \
STATS_OUTPUT_DIR="${install_root}/var/www" \
STATS_SYSTEMD_DIR="${install_root}/systemd" \
STATS_LOGROTATE_DIR="${install_root}/logrotate" \
STATS_SYSTEMCTL="${mock_bin}/systemctl" \
bash "${bundle_dir}/deploy/install.sh"

[[ -x ${install_root}/bin/stats ]]
[[ -x ${install_root}/bin/stats-report-worker ]]
[[ -f ${install_root}/systemd/stats-update.service ]]
[[ -f ${install_root}/systemd/stats-update.timer ]]
[[ -f ${install_root}/logrotate/stats ]]
grep -q 'TimeoutStartSec=' "${install_root}/systemd/stats-update.service"
grep -q 'create 0640 root stats' "${install_root}/logrotate/stats"

echo "installer smoke test passed"
