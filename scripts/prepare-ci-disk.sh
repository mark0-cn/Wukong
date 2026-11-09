#!/usr/bin/env bash
set -euo pipefail
# This script is deliberately limited to disposable GitHub-hosted Linux VMs.
[[ "${GITHUB_ACTIONS:-}" == true && "${RUNNER_ENVIRONMENT:-}" == github-hosted && "${RUNNER_OS:-}" == Linux ]] || {
  echo 'Disk preparation only runs on GitHub-hosted Linux runners.' >&2; exit 1;
}
df -h /
# Neither the Docker build nor its acceptance job needs these preinstalled SDKs.
for directory in /usr/local/lib/android /usr/share/dotnet /opt/ghc /usr/local/.ghcup /opt/hostedtoolcache/CodeQL; do
  if [[ -d "$directory" && ! -L "$directory" ]]; then
    sudo rm -rf -- "$directory"
  fi
done
available=$(df -Pk / | awk 'NR==2 {print $4}')
minimum_gib=${1:-25}
if (( available < minimum_gib * 1024 * 1024 )); then
  echo "Need at least ${minimum_gib} GiB free. Select a larger hosted runner using WORKBENCH_LINUX_RUNNER." >&2
  exit 1
fi
df -h /
