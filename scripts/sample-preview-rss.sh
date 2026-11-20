#!/usr/bin/env bash
# Samples the resident memory of the preview app's `web` process while the
# e2e suite runs, so memory is measured rather than guessed (ticket #65).
#
#   scripts/sample-preview-rss.sh gazebo-pr-123 > rss-before.csv   # then run the suite
#
# Output: ISO time, VmRSS kB, VmHWM kB (peak), until interrupted. Also check
# for OOM kills afterwards:  fly logs -a <app> --no-tail | grep 'Out of memory'
set -euo pipefail
app="${1:?usage: $0 <fly-app> [interval-seconds]}"
interval="${2:-2}"
echo "time,vmrss_kb,vmhwm_kb"
while true; do
  line=$(fly ssh console -a "$app" -C 'sh -c "pid=\$(pidof web | cut -d\" \" -f1); grep -E \"VmRSS|VmHWM\" /proc/\$pid/status | tr -s \" \" | cut -d\" \" -f2 | tr \"\\n\" \" \""' 2>/dev/null || true)
  set -- $line
  echo "$(date -u +%FT%TZ),${1:-},${2:-}"
  sleep "$interval"
done
