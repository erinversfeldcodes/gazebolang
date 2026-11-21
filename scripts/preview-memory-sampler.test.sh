#!/bin/sh
# Tests the sampler's write decision on synthetic readings (ticket #80).
# Run: sh scripts/preview-memory-sampler.test.sh
here=$(cd "$(dirname "$0")" && pwd)
SAMPLER_SOURCE_ONLY=1
. "$here/preview-memory-sampler.sh"

fails=0
check() { if [ "$2" = "$3" ]; then echo "ok   - $1"; else echo "FAIL - $1: expected '$2', got '$3'"; fails=$((fails+1)); fi; }
reset() { last_rss=""; last_t=0; max_rss=0; min_avail=""; LINES=""; count=0; }
# feed RSS AVAIL T: one reading at second T
feed() { if decide "$1" "$1" "$2" "$3" "$3"; then count=$((count+1)); LINES="$LINES
$LINE"; fi; }

reset
t=1000; while [ $t -lt 1060 ]; do feed 200000 500000 $t; t=$((t+1)); done
# first reading, then heartbeats at +15, +30, +45 (60 s window)
check "steady memory writes first line plus a heartbeat every 15 s" 4 $count

reset
t=1000; while [ $t -lt 1010 ]; do feed 200000 500000 $t; t=$((t+1)); done
feed 210240 500000 1010
check "a step of exactly 10240 kB writes one line" 2 $count
feed 210241 500000 1011
check "a small change after it writes nothing" 2 $count
feed 200000 500000 1012
check "a drop of 10240 kB or more writes a line" 3 $count

reset
feed 200000 500000 1000
feed 205000 500000 1001
feed 209000 500000 1002
check "steps under 10240 kB from the last line write nothing" 1 $count

reset
feed 200000 500000 1000
t=1001; while [ $t -lt 1011 ]; do feed 200000 65535 $t; t=$((t+1)); done
check "MemAvailable under 64 MB writes every reading" 11 $count
feed 200000 65536 1011
check "MemAvailable of exactly 64 MB is not low" 11 $count

reset
feed 200000 500000 1000
feed 205000 400000 1001
feed 209000 300000 1002
feed 202000 450000 1003
feed 211000 350000 1004
last=$(printf '%s\n' "$LINES" | tail -n 1)
case "$last" in *" max_rss_kb=211000 min_avail_kb=300000") r=ok;; *) r="$last";; esac
check "max_rss_kb and min_avail_kb are the extremes since the previous line" ok "$r"
feed 200000 500000 1005
last=$(printf '%s\n' "$LINES" | tail -n 1)
case "$last" in *" max_rss_kb=200000 min_avail_kb=500000") r=ok;; *) r="$last";; esac
check "extremes reset after a line is written" ok "$r"

reset
feed 200000 500000 1000
case "$LINE" in "RSS_SAMPLE uptime_s=1000 epoch_s=1000 rss_kb=200000 hwm_kb=200000 memavailable_kb=500000 max_rss_kb=200000 min_avail_kb=500000") r=ok;; *) r="$LINE";; esac
check "line keeps its fields and adds the two new ones" ok "$r"

[ $fails -eq 0 ] || { echo "$fails failed"; exit 1; }
echo "all passed"
