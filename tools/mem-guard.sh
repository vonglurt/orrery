#!/bin/sh
# mem-guard.sh COMMAND [ARGS...] -- run a command and stop it before it
# takes the Mac's memory. Every 2 seconds it reads the resident memory of the
# command and everything it started, and the system's free-memory percentage
# (memory_pressure -Q). Past MAX_MB (default 4096) for the command, or under
# MIN_FREE percent (default 20) for the system, the whole process group is
# stopped (TERM, then KILL) and the guard exits 137. At the end it prints the
# peak and the lowest free percentage it saw.
#
# Written 28 October 2026 after an orrery release build (lto, one codegen
# unit) ran beside eight census processes and ran this Mac out of memory.
# tools/mac-app.sh builds under it.
MAX_MB=${MAX_MB:-4096}
MIN_FREE=${MIN_FREE:-20}
[ $# -gt 0 ] || { echo "usage: mem-guard.sh COMMAND [ARGS...]" >&2; exit 2; }

# Its own process group, so one signal reaches cargo, rustc and the linker.
set -m
"$@" &
pid=$!
set +m

tree_mb() {
    ps -ax -o pid=,ppid=,rss= | awk -v root="$1" '
        { parent[$1] = $2; rss[$1] = $3 }
        END {
            for (p in rss) {
                q = p
                while (q != "" && q != 0 && q != 1) {
                    if (q == root) { sum += rss[p]; break }
                    q = parent[q]
                }
            }
            printf "%d", sum / 1024
        }'
}
free_pct() {
    memory_pressure -Q 2>/dev/null | sed -n 's/.*percentage: \([0-9]*\)%.*/\1/p'
}

peak=0
low=100
stopped=""
while kill -0 "$pid" 2>/dev/null; do
    mb=$(tree_mb "$pid")
    fr=$(free_pct)
    [ -n "$fr" ] || fr=100
    [ "$mb" -gt "$peak" ] && peak=$mb
    [ "$fr" -lt "$low" ] && low=$fr
    if [ "$mb" -gt "$MAX_MB" ]; then
        stopped="it held ${mb} MB, past MAX_MB=${MAX_MB}"
    elif [ "$fr" -lt "$MIN_FREE" ]; then
        stopped="the system was down to ${fr}% free, under MIN_FREE=${MIN_FREE}"
    fi
    if [ -n "$stopped" ]; then
        kill -TERM -"$pid" 2>/dev/null
        sleep 3
        kill -KILL -"$pid" 2>/dev/null
        wait "$pid" 2>/dev/null
        echo "mem-guard: stopped '$*': $stopped (peak ${peak} MB, lowest free ${low}%)" >&2
        exit 137
    fi
    sleep 2
done
wait "$pid"
rc=$?
echo "mem-guard: '$*' exited $rc; peak ${peak} MB, lowest free ${low}%" >&2
exit $rc
