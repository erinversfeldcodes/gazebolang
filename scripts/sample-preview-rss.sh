#!/usr/bin/env bash
# Samples the resident memory of the preview app's `web` process while the
# e2e suite runs, so memory is measured rather than guessed (ticket #65).
#
#   scripts/sample-preview-rss.sh gazebo-pr-123 > rss-before.csv   # then run the suite
#
# Output: ISO time, VmRSS kB, VmHWM kB (peak), until interrupted. Failed samples
# are reported on stderr and written as empty rows. Also check
# for OOM kills afterwards:  fly logs -a <app> --no-tail | grep 'Out of memory'
set -euo pipefail
app="${1:?usage: $0 <fly-app> [interval-seconds]}"
interval="${2:-2}"
echo "time,vmrss_kb,vmhwm_kb"
remote='pid=$(pidof web | cut -d" " -f1); [ -n "$pid" ] || exit 3; awk "/^VmRSS/{r=\$2}/^VmHWM/{h=\$2}END{print r\",\"h}" /proc/$pid/status'
while true; do
  now=$(date -u +%FT%TZ)
  if row=$(fly ssh console -a "$app" -C "sh -c '$remote'" 2>&1); then
    echo "$now,$(printf '%s' "$row" | tail -n1 | tr -d '\r')"
  else
    echo "sample failed at $now: $row" >&2
    echo "$now,,"
  fi
  sleep "$interval"
done
