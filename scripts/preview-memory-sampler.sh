#!/bin/sh
# Memory sampler that runs ON the preview machine (tickets #71, #73, #80, for
# #65). Started detached by .github/workflows/preview.yml. POSIX sh: the
# machine has busybox-style tools only.
#
# It reads VmRSS of the `web` process and MemAvailable every second, but writes
# an RSS_SAMPLE line to /proc/1/fd/1 (the machine's log stream) only when:
#   - VmRSS differs from the last line written by 10240 kB or more;
#   - MemAvailable is under 65536 kB (every reading is then written);
#   - 15 s have passed since the last line (heartbeat).
# Fly keeps only the last 100 log lines per app, so one line a second would push
# out the app's own output. Each line carries max_rss_kb and min_avail_kb, the
# extremes of the readings since the previous line, so the run's peak and floor
# are not lost between lines.
#
# Sourcing with SAMPLER_SOURCE_ONLY=1 defines the functions only (used by
# scripts/preview-memory-sampler.test.sh).

RSS_STEP_KB=10240
LOW_AVAIL_KB=65536
HEARTBEAT_S=15

last_rss=""
last_t=0
max_rss=0
min_avail=""

# decide RSS HWM AVAIL NOW UPTIME: folds one reading into the running extremes
# and, when it is time to write, sets LINE and returns 0; otherwise returns 1.
decide() {
  d_rss=$1; d_hwm=$2; d_av=$3; d_now=$4; d_up=$5
  if [ "$d_rss" -gt "$max_rss" ]; then max_rss=$d_rss; fi
  if [ -z "$min_avail" ] || [ "$d_av" -lt "$min_avail" ]; then min_avail=$d_av; fi
  write=0
  if [ -z "$last_rss" ]; then
    write=1
  else
    diff=$((d_rss - last_rss))
    if [ "$diff" -lt 0 ]; then diff=$((-diff)); fi
    if [ "$diff" -ge "$RSS_STEP_KB" ]; then write=1; fi
    if [ "$d_av" -lt "$LOW_AVAIL_KB" ]; then write=1; fi
    if [ $((d_now - last_t)) -ge "$HEARTBEAT_S" ]; then write=1; fi
  fi
  [ "$write" -eq 1 ] || return 1
  LINE="RSS_SAMPLE uptime_s=$d_up epoch_s=$d_now rss_kb=$d_rss hwm_kb=$d_hwm memavailable_kb=$d_av max_rss_kb=$max_rss min_avail_kb=$min_avail"
  last_rss=$d_rss
  last_t=$d_now
  max_rss=0
  min_avail=""
  return 0
}

run_sampler() {
  n=0
  pid=""
  while [ $n -lt 2400 ]; do
    # Scan /proc only when the cached pid is gone or no longer `web`, so a
    # steady-state sample forks little on a small machine.
    if [ -z "$pid" ] || [ "$(cat /proc/$pid/comm 2>/dev/null)" != web ]; then
      pid=""
      for d in /proc/[0-9]*; do
        if [ "$(cat $d/comm 2>/dev/null)" = web ]; then pid=${d#/proc/}; break; fi
      done
    fi
    st="0 0"
    if [ -n "$pid" ]; then
      st=$(awk '/^VmRSS/{r=$2}/^VmHWM/{h=$2}END{print r+0 " " h+0}' /proc/$pid/status 2>/dev/null)
    fi
    [ -n "$st" ] || st="0 0"
    set -- $st
    up=$(cut -d' ' -f1 /proc/uptime)
    av=$(awk '/^MemAvailable/{print $2}' /proc/meminfo)
    if decide "$1" "$2" "$av" "$(date +%s)" "$up"; then
      echo "$LINE" > /proc/1/fd/1
    fi
    n=$((n+1))
    sleep 1
  done
}

if [ -z "$SAMPLER_SOURCE_ONLY" ]; then run_sampler; fi
