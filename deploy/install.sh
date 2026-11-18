#!/usr/bin/env bash
set -euo pipefail

if [[ ${EUID} -ne 0 ]]; then
    echo "run as root" >&2
    exit 1
fi

project_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
stats_user=${STATS_USER:-stats}
stats_group=${STATS_GROUP:-stats}
binary_path=${STATS_BINARY_PATH:-/opt/stats/bin/stats}
config_path=${STATS_CONFIG_PATH:-/opt/stats/etc/awstats.example.test.conf}
data_dir=${STATS_DATA_DIR:-/opt/stats/var/lib}
output_dir=${STATS_OUTPUT_DIR:-/opt/stats/var/www}
systemd_dir=${STATS_SYSTEMD_DIR:-/etc/systemd/system}
logrotate_dir=${STATS_LOGROTATE_DIR:-/etc/logrotate.d}
systemctl_bin=${STATS_SYSTEMCTL:-systemctl}

if [[ -f ${project_dir}/stats ]]; then
    source_binary=${project_dir}/stats
    source_worker=${project_dir}/stats-report-worker
elif [[ -f ${project_dir}/target/release/stats-cli ]]; then
    source_binary=${project_dir}/target/release/stats-cli
    source_worker=${project_dir}/target/release/stats-report-worker
else
    echo "could not find stats binary in ${project_dir}" >&2
    exit 1
fi
if [[ ! -f ${source_worker} ]]; then
    echo "could not find stats-report-worker next to ${source_binary}" >&2
    exit 1
fi
source_config=${STATS_CONFIG_SOURCE:-${project_dir}/awstats.example.test.conf}
if [[ ! -f ${source_config} && -f ${project_dir}/examples/awstats.example.test.conf ]]; then
    source_config=${project_dir}/examples/awstats.example.test.conf
fi
if [[ ! -f ${source_config} ]]; then
    echo "could not find configuration source ${source_config}" >&2
    exit 1
fi

if ! id "${stats_user}" >/dev/null 2>&1; then
    useradd --system --home-dir /opt/stats --no-create-home --shell /usr/sbin/nologin "${stats_user}"
fi

if ! getent group "${stats_group}" >/dev/null 2>&1; then
    groupadd --system "${stats_group}"
fi

install -D -m 0755 "${source_binary}" "${binary_path}"
install -D -m 0755 "${source_worker}" "$(dirname "${binary_path}")/stats-report-worker"
install -d -o "${stats_user}" -g "${stats_group}" "${data_dir}" "${output_dir}"
install -d -o root -g root "$(dirname "${config_path}")"
if [[ ! -e ${config_path} && ! -L ${config_path} ]]; then
    install -m 0644 "${source_config}" "${config_path}"
    echo "installed example configuration ${config_path}"
fi
install -d "${systemd_dir}" "${logrotate_dir}"
service_tmp=$(mktemp)
trap 'rm -f "${service_tmp}"' EXIT
sed \
    -e "s|@STATS_BINARY_PATH@|${binary_path}|g" \
    -e "s|@STATS_CONFIG_PATH@|${config_path}|g" \
    -e "s|@STATS_DATA_DIR@|${data_dir}|g" \
    -e "s|@STATS_OUTPUT_DIR@|${output_dir}|g" \
    "${project_dir}/deploy/stats-update.service" >"${service_tmp}"
install -m 0644 "${service_tmp}" "${systemd_dir}/stats-update.service"
install -m 0644 "${project_dir}/deploy/stats-update.timer" "${systemd_dir}/stats-update.timer"
install -m 0644 "${project_dir}/deploy/stats-logrotate" "${logrotate_dir}/stats"

"${systemctl_bin}" daemon-reload
if [[ ${STATS_ENABLE_TIMER:-0} == 1 ]]; then
    "${systemctl_bin}" enable --now stats-update.timer
else
    echo "timer left disabled; enable it with: systemctl enable --now stats-update.timer"
fi

echo "installed ${binary_path}"
echo "configuration path: ${config_path}"
echo "data directory: ${data_dir}"
echo "output directory: ${output_dir}"
echo "review the configuration, then run 'systemctl start --wait stats-update.service'"
