#!/usr/bin/env bash
set -euo pipefail

if [[ ${EUID} -ne 0 ]]; then
    echo "run as root" >&2
    exit 1
fi

project_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
stats_user=${STATS_USER:-stats}
stats_group=${STATS_GROUP:-stats}
binary_path=${STATS_BINARY_PATH:-/usr/local/bin/stats}
config_path=${STATS_CONFIG_PATH:-/etc/stats/awstats.example.test.conf}
data_dir=${STATS_DATA_DIR:-/var/lib/stats}
output_dir=${STATS_OUTPUT_DIR:-/var/www/stats}

if ! id "${stats_user}" >/dev/null 2>&1; then
    useradd --system --home-dir /var/lib/stats --no-create-home --shell /usr/sbin/nologin "${stats_user}"
fi

if ! getent group "${stats_group}" >/dev/null 2>&1; then
    groupadd --system "${stats_group}"
fi

install -D -m 0755 "${project_dir}/target/release/stats-cli" "${binary_path}"
install -d -o "${stats_user}" -g "${stats_group}" "${data_dir}" "${output_dir}"
install -d -o root -g root "$(dirname "${config_path}")"
install -m 0644 "${project_dir}/deploy/stats-update.service" /etc/systemd/system/stats-update.service
install -m 0644 "${project_dir}/deploy/stats-update.timer" /etc/systemd/system/stats-update.timer
install -m 0644 "${project_dir}/deploy/stats-logrotate" /etc/logrotate.d/stats

systemctl daemon-reload
systemctl enable --now stats-update.timer

echo "installed ${binary_path}"
echo "configuration expected at ${config_path}"
echo "data directory: ${data_dir}"
echo "output directory: ${output_dir}"
echo "run 'systemctl start --wait stats-update.service' after installing the configuration"
