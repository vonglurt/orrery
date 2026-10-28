#!/bin/bash
# topnotch: a fleet node at a glance, htop-like, in one screen of text.
# Installed by tools/fleet-login.sh as /usr/local/bin/topnotch and run by the
# login banner (/etc/update-motd.d/01-copal-fleet); run it any time, or keep
# it live:  watch -c -n 2 topnotch    (copal-tm is the full task manager;
# topnotch is the glance.)
#
#   the node      its name, role (/etc/copal-fleet/node.conf), system, uptime,
#                 and address
#   the machine   CPU, memory and disk, as bars
#   the GPU       load, memory and temperature, and what is running on it
#   busy now      every process using more than 10% of a core NOW
#   remote work   who is in over SSH and from where; and what each SSH
#                 connection has started -- a script, a build, a training run
#                 -- with its user, source address, age and CPU now
#
# "Now" is measured: every process's CPU time is read twice, half a second
# apart, as htop does. ps's %CPU is a process's average over its whole life,
# so a job that went flat out a minute ago, after an hour of waiting, barely
# shows there.
#
# As root (the login banner runs as root) every process can be tied to its
# SSH connection; as an ordinary user, connections whose sshd is not yours
# show as "?".
set -u
export LC_ALL=C.UTF-8

ROLE=''; HOLD=''; WARN=''; HINTS=()
if [ -r /etc/copal-fleet/node.conf ]; then
    while IFS= read -r line; do
        case "$line" in
            ROLE=*) ROLE=${line#ROLE=}; ROLE=${ROLE//\"/} ;;
            HOLD=*) HOLD=${line#HOLD=}; HOLD=${HOLD//\"/} ;;
            WARN=*) WARN=${line#WARN=}; WARN=${WARN//\"/} ;;
            HINT=*) h=${line#HINT=}; HINTS+=("${h//\"/}") ;;
        esac
    done < /etc/copal-fleet/node.conf
fi
BRIEF=0; [ "${1:-}" = --brief ] && BRIEF=1

B=$'\033[1m'; DIM=$'\033[2m'; Y=$'\033[33m'; G=$'\033[32m'; R=$'\033[31m'; C=$'\033[36m'; Z=$'\033[0m'
# bar PERCENT WIDTH: a bar coloured by how full it is
bar() {
    local p=${1%.*} w=${2:-24} n i s=''
    [ -z "$p" ] && p=0; [ "$p" -gt 100 ] && p=100; [ "$p" -lt 0 ] && p=0
    n=$(( (p * w + 50) / 100 ))
    for ((i = 0; i < w; i++)); do if [ "$i" -lt "$n" ]; then s+='█'; else s+='░'; fi; done
    local col=$G; [ "$p" -ge 60 ] && col=$Y; [ "$p" -ge 85 ] && col=$R
    printf '%s%s%s %3d%%' "$col" "$s" "$Z" "$p"
}

. /etc/os-release
host=$(hostname)
up=$(uptime -p 2>/dev/null | sed 's/^up //')
printf '\n  %sCopal fleet%s %s· orrery%s\n' "$B" "$Z" "$DIM" "$Z"
printf '  %s%s%s -- %s %s(%s, not Copal; up %s)%s\n' "$B" "$host" "$Z" "${ROLE:-a fleet node}" "$DIM" "$PRETTY_NAME" "$up" "$Z"
addr=$(ip -4 -o addr show scope global 2>/dev/null | awk '{split($4,a,"/"); printf "%s%s (%s)", (n++ ? ", " : ""), a[1], $2}')
printf '  addr  %s\n' "${addr:-no IPv4 address}"

# ---- the machine ---------------------------------------------------------------
# Every process's CPU ticks (utime + stime, fields 14 and 15 of
# /proc/PID/stat, read after the ")" that ends the name, which may hold
# spaces), and the machine's, twice, half a second apart.
ticks() { awk '{ l = $0; sub(/^.*\) /, "", l); split(l, a, " "); split(FILENAME, f, "/"); print f[3], a[12] + a[13] }' /proc/[0-9]*/stat 2>/dev/null; }
declare -A T0 NOW
while read -r p t; do T0[$p]=$t; done < <(ticks)
read -r _ a1 b1 c1 d1 e1 f1 g1 _ < /proc/stat; sleep 0.5
read -r _ a2 b2 c2 d2 e2 f2 g2 _ < /proc/stat
HZ=$(getconf CLK_TCK 2>/dev/null || echo 100)
while read -r p t; do [ -n "${T0[$p]:-}" ] && NOW[$p]=$(( (t - T0[$p]) * 100 * 2 / HZ )); done < <(ticks)
busy=$(( (a2+b2+c2+f2+g2) - (a1+b1+c1+f1+g1) )); idle=$(( (d2+e2) - (d1+e1) ))
cpu=$(( busy + idle > 0 ? 100 * busy / (busy + idle) : 0 ))
read -r l1 l5 l15 _ < /proc/loadavg
printf '\n  CPU   %s   load %s %s %s · %s cores\n' "$(bar "$cpu")" "$l1" "$l5" "$l15" "$(nproc)"
mt=$(awk '/^MemTotal/{print $2}' /proc/meminfo); ma=$(awk '/^MemAvailable/{print $2}' /proc/meminfo)
mu=$(( mt - ma ))
printf '  mem   %s   %s of %s GB\n' "$(bar $(( 100 * mu / mt )))" "$(awk -v k="$mu" 'BEGIN{printf "%.1f", k/1048576}')" "$(awk -v k="$mt" 'BEGIN{printf "%.0f", k/1048576}')"
read -r dsz dus dpc <<<"$(df -BG --output=size,used,pcent / | tail -1 | tr -d 'G%')"
printf '  disk  %s   %s of %s GB on /\n' "$(bar "$dpc")" "$dus" "$dsz"
# The update count, from the file Ubuntu's own login message reads (written
# by apt's update-notifier), without the ESM advertising around it.
upd=$(grep -h 'can be applied immediately' /var/lib/update-notifier/updates-available 2>/dev/null | head -n1)
[ -n "$upd" ] && printf '  apt   %s\n' "$upd"
[ -f /var/run/reboot-required ] && printf '  %sa reboot is required%s\n' "$Y" "$Z"

# ---- the GPU ---------------------------------------------------------------------
if command -v nvidia-smi >/dev/null 2>&1; then
    g=$(nvidia-smi --query-gpu=name,driver_version,temperature.gpu,utilization.gpu,memory.used,memory.total,power.draw --format=csv,noheader,nounits 2>/dev/null | head -n1)
    if [ -n "$g" ]; then
        IFS=',' read -r name drv temp util gmu gmt pw <<<"$g"
        name=${name# }; drv=${drv# }; temp=${temp# }; util=${util# }; gmu=${gmu# }; gmt=${gmt# }; pw=${pw# }
        printf '\n  GPU   %s%s%s · driver %s · %s°C · %s W\n' "$C" "$name" "$Z" "$drv" "$temp" "${pw%.*}"
        printf '        load %s   mem %s   %s of %s MiB\n' "$(bar "$util" 16)" "$(bar $(( gmt > 0 ? 100 * gmu / gmt : 0 )) 16)" "$gmu" "$gmt"
        apps=$(nvidia-smi --query-compute-apps=pid,process_name,used_memory --format=csv,noheader,nounits 2>/dev/null)
        if [ -n "$apps" ]; then
            while IFS=',' read -r ap an am; do
                printf '        %son the GPU:%s %s %s (%s MiB)\n' "$B" "$Z" "${ap# }" "$(basename "${an# }")" "${am# }"
            done <<<"$apps"
        else
            printf '        %snothing on the GPU%s\n' "$DIM" "$Z"
        fi
    else
        printf '\n  GPU   %snvidia-smi gives no answer: is the driver loaded?%s\n' "$Y" "$Z"
    fi
    for c in /usr/local/cuda/bin/nvcc /usr/local/cuda-*/bin/nvcc; do
        [ -x "$c" ] || continue
        printf '  CUDA  %s\n' "$("$c" --version 2>/dev/null | sed -n 's/.*release \([0-9.]*\).*/\1/p')"; break
    done
fi
[ -n "$HOLD" ] && printf '  %sheld: %s%s\n' "$DIM" "$HOLD" "$Z"

# ---- remote work -----------------------------------------------------------------
# sshd process -> the address it serves, from the established connections on :22
declare -A FROM
while read -r peer procs; do
    for p in $(grep -o 'pid=[0-9]*' <<<"$procs" | cut -d= -f2); do FROM[$p]=${peer%:*}; done
done < <(ss -tnpH state established '( sport = :22 )' 2>/dev/null | awk '{print $4, $6}')
# every process: pid ppid user seconds cpu command
declare -A PPID_OF CMD_OF
while read -r pid ppid _; do PPID_OF[$pid]=$ppid; done < <(ps -eo pid=,ppid=)
# the SSH connection a process belongs to. First its own environment:
# SSH_CONNECTION="client port server port", inherited by everything an SSH
# session starts, and readable for one's own processes (and as root, for
# all). Then the nearest sshd ancestor that ss names, which needs root.
conn_of() {
    local p=$1 n=0 e
    e=$(tr '\0' '\n' < "/proc/$p/environ" 2>/dev/null | sed -n 's/^SSH_CONNECTION=\([^ ]*\) .*/\1/p')
    [ -n "$e" ] && { echo "$e"; return; }
    while [ -n "$p" ] && [ "$p" != 1 ] && [ "$p" != 0 ] && [ $n -lt 40 ]; do
        [ -n "${FROM[$p]:-}" ] && { echo "${FROM[$p]}"; return; }
        p=${PPID_OF[$p]:-}; n=$((n + 1))
    done
}
under_sshd() {
    local p=$1 n=0 c
    while [ -n "$p" ] && [ "$p" != 1 ] && [ "$p" != 0 ] && [ $n -lt 40 ]; do
        c=$(cat "/proc/$p/comm" 2>/dev/null); [ "$c" = sshd ] || [ "$c" = sshd-session ] && return 0
        p=${PPID_OF[$p]:-}; n=$((n + 1))
    done
    return 1
}

sess=$(who 2>/dev/null | awk '$5 ~ /\(/ {gsub(/[()]/,"",$5); print $1, $5, $4}' | sort | uniq -c)
printf '\n  %sRemote sessions%s\n' "$B" "$Z"
if [ -n "$sess" ]; then
    while read -r cnt u ip since; do printf '    %-10s from %-16s since %s  (%s login%s)\n' "$u" "$ip" "$since" "$cnt" "$([ "$cnt" = 1 ] || echo s)"; done <<<"$sess"
else
    printf '    %sno interactive logins%s\n' "$DIM" "$Z"
fi

me=$$
mysid=$(ps -o sess= -p $$ | tr -d ' ')
rows=''
while read -r pid sid user et pc args; do
    [ "$pid" = "$me" ] && continue
    # this screen's own session (its pipes, a sed, the login that runs it)
    [ "$sid" = "$mysid" ] && continue
    # sshd itself, and bare interactive shells: their children are what counts
    case "$args" in
        sshd*|"sshd-session"*|"/usr/lib/openssh/"*|-bash|-sh|-zsh|bash|sh|zsh|/bin/bash|/bin/sh) continue ;;
        *topnotch*|*fleet-status*|*01-copal-fleet*|*run-parts*|*update-motd*) continue ;;
    esac
    under_sshd "$pid" || continue
    from=$(conn_of "$pid"); from=${from:-?}
    h=$(( et / 3600 )); m=$(( et % 3600 / 60 )); s=$(( et % 60 ))
    age=$( [ $h -gt 0 ] && printf '%dh%02dm' $h $m || printf '%dm%02ds' $m $s )
    now=${NOW[$pid]:-0}
    rows+=$(printf '%06d    %-7s %-8s %-16s %7s %5s%%  %s' "$now" "$pid" "$user" "$from" "$age" "$now" "${args:0:60}")$'\n'
done < <(ps -eo pid=,sess=,user=,etimes=,pcpu=,args=)
printf '\n  %sStarted over SSH%s %s(scripts, builds, runs; not the shells themselves)%s\n' "$B" "$Z" "$DIM" "$Z"
if [ -n "$rows" ]; then
    printf '    %s%-7s %-8s %-16s %7s %6s  %s%s\n' "$DIM" PID USER FROM AGE NOW COMMAND "$Z"
    printf '%s' "$rows" | sort -r | cut -c7- | head -n 12
else
    printf '    %snothing running from a remote session%s\n' "$DIM" "$Z"
fi

# ---- busy now --------------------------------------------------------------------
# Every process over 10% of a core in the half second measured, busiest first.
busy=''
for p in "${!NOW[@]}"; do
    [ "${NOW[$p]}" -ge 10 ] || continue
    [ "$p" = "$me" ] && continue
    read -r u pm a < <(ps -o user=,pmem=,args= -p "$p" 2>/dev/null) || continue
    case "$a" in *topnotch*|"ps "*) continue ;; esac
    f=$(under_sshd "$p" && conn_of "$p")
    busy+=$(printf '%06d    %-7s %-8s %5s%% %5s%%  %-16s %s' "${NOW[$p]}" "$p" "$u" "${NOW[$p]}" "$pm" "${f:--}" "${a:0:56}")$'\n'
done
printf '\n  %sBusy now%s %s(over 10%% of a core, measured over half a second)%s\n' "$B" "$Z" "$DIM" "$Z"
if [ -n "$busy" ]; then
    printf '    %s%-7s %-8s %6s %6s  %-16s %s%s\n' "$DIM" PID USER CPU MEM FROM COMMAND "$Z"
    printf '%s' "$busy" | sort -r | cut -c7- | head -n "$([ "$BRIEF" = 1 ] && echo 5 || echo 10)"
else
    printf '    %snothing above 10%% of a core%s\n' "$DIM" "$Z"
fi

if [ "${#HINTS[@]}" -gt 0 ]; then
    printf '\n  Hints\n'
    for h in "${HINTS[@]}"; do printf '    %s\n' "$h"; done
    printf '    %-32s %s\n' "watch -c -n 2 topnotch" "this screen, live"
    if command -v copal-tm >/dev/null 2>&1 || ls /home/*/.cargo/bin/copal-tm >/dev/null 2>&1; then
        printf '    %-32s %s\n' "copal-tm" "the full task manager: every process, signals, ports"
    else
        printf '    %-32s %s\n' "cargo install copal-tm" "the full task manager (not installed yet)"
    fi
fi
[ -n "$WARN" ] && printf '\n  %s%s%s\n' "$Y" "$WARN" "$Z"
printf '\n'
