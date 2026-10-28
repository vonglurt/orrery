#!/bin/bash
# fleet-train: rounds of work on the fleet, each one judged, until told to stop.
#
#   tools/fleet-train.sh run STAGES [ROUNDS]   run rounds (all of them, or ROUNDS)
#   tools/fleet-train.sh status STAGES [--json]
#   tools/fleet-train.sh stop STAGES           finish the round in hand, then stop
#
# A project describes its rounds in a STAGES file, a bash file this script
# sources. It sets FT_DIR (where the loop keeps its state) and FT_STAGES (the
# stage names, in order), and defines one function per stage, stage_<name>,
# which is given the round number and the round's folder and returns 0 when
# the stage did its work. treadcore's is scripts/fleet-train-stages.sh: record
# on every spare CPU, train on the GPU, judge by a census, keep the best.
#
# The loop owns only what any project's would: rounds, stages, the event
# stream, and stopping. Everything it knows is in files a person can read:
#
#   FT_DIR/round        the number of the round in hand (or the last)
#   FT_DIR/STOP         present: stop after the round in hand
#   FT_DIR/events.jsonl a line an event: {"t","round","stage","event","detail"}
#   FT_DIR/rounds/NNN/  each round's own folder: its stages' logs and results,
#                       and summary.txt, one line the round's verdict
#   FT_DIR/best         what the project's judge keeps as the best so far
#
# `status --json` prints one document of all of it: the shape orrery's
# museum interface reads to show the jobs (docs/fleet-train.md). A stage that
# fails ends the round as failed and the loop stops: a failure is for a
# person to read, not for a loop to retry around.
set -euo pipefail

usage() { sed -n '4,7p' "$0" >&2; exit 2; }
[ $# -ge 2 ] || usage
CMD="$1" STAGES="$2"
shift 2
[ -f "$STAGES" ] || { echo "no stages file $STAGES" >&2; exit 2; }
# shellcheck disable=SC1090
. "$STAGES"
: "${FT_DIR:?the stages file must set FT_DIR}" "${FT_STAGES:?the stages file must set FT_STAGES}"
mkdir -p "$FT_DIR/rounds"
EV="$FT_DIR/events.jsonl"

# A JSON string, escaped (bash 3.2: no ${x//} on special characters needed here
# beyond backslash, quote and the control characters a log line can carry).
js() { printf '%s' "$1" | python3 -c 'import json,sys; print(json.dumps(sys.stdin.read()), end="")'; }
event() {  # ROUND STAGE EVENT DETAIL
    printf '{"t":"%s","round":%s,"stage":%s,"event":%s,"detail":%s}\n' \
        "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$1" "$(js "$2")" "$(js "$3")" "$(js "$4")" >> "$EV"
}

case "$CMD" in
run)
    LIMIT="${1:-0}"
    rm -f "$FT_DIR/STOP"
    done_here=0
    while :; do
        if [ -f "$FT_DIR/STOP" ]; then event "$(cat "$FT_DIR/round" 2>/dev/null || echo 0)" loop stopped "STOP found"; break; fi
        if [ "$LIMIT" -gt 0 ] && [ "$done_here" -ge "$LIMIT" ]; then break; fi
        R=$(( $(cat "$FT_DIR/round" 2>/dev/null || echo 0) + 1 ))
        echo "$R" > "$FT_DIR/round"
        RD="$FT_DIR/rounds/$(printf '%03d' "$R")"
        mkdir -p "$RD"
        event "$R" round started ""
        echo "== round $R  $(date '+%F %T')"
        for s in $FT_STAGES; do
            event "$R" "$s" started ""
            echo "   $s"
            t0=$(date +%s)
            if "stage_$s" "$R" "$RD" > "$RD/$s.log" 2>&1; then
                event "$R" "$s" done "$(( $(date +%s) - t0 )) s; $(tail -1 "$RD/$s.log" | cut -c1-200)"
            else
                event "$R" "$s" failed "$(tail -1 "$RD/$s.log" | cut -c1-200)"
                echo "round $R: $s failed: see $RD/$s.log" | tee "$RD/summary.txt"
                event "$R" round failed "$s"
                exit 1
            fi
        done
        [ -f "$RD/summary.txt" ] || echo "round $R: done" > "$RD/summary.txt"
        event "$R" round done "$(cat "$RD/summary.txt")"
        echo "   $(cat "$RD/summary.txt")"
        done_here=$(( done_here + 1 ))
    done
    ;;

stop)
    touch "$FT_DIR/STOP"
    echo "the loop stops after the round in hand ($FT_DIR/STOP)"
    ;;

status)
    if [ "${1:-}" = --json ]; then
        # A stages file may say more about the round in hand (treadcore's
        # gives each machine's lane and the pipe it is running): ft_detail
        # prints a JSON object, merged into the document.
        detail='{}'
        if declare -F ft_detail > /dev/null; then detail="$(ft_detail 2>/dev/null || echo '{}')"; fi
        FT_DIR="$FT_DIR" FT_STAGES="$FT_STAGES" FT_DETAIL="$detail" python3 - <<'EOF'
import json, os
d = os.environ["FT_DIR"]; stages = os.environ["FT_STAGES"].split()
ev = []
try:
    ev = [json.loads(l) for l in open(os.path.join(d, "events.jsonl")) if l.strip()]
except FileNotFoundError:
    pass
rounds = {}
for e in ev:
    r = rounds.setdefault(e["round"], {"round": e["round"], "state": "running", "stages": {}, "started": None, "ended": None})
    if e["stage"] == "round":
        if e["event"] == "started": r["started"] = e["t"]
        else: r["state"], r["ended"] = e["event"], e["t"]
    elif e["stage"] != "loop":
        st = r["stages"].setdefault(e["stage"], {})
        st["state"] = e["event"]; st[e["event"]] = e["t"]; st["detail"] = e["detail"]
for r in rounds.values():
    p = os.path.join(d, "rounds", "%03d" % r["round"], "summary.txt")
    if os.path.exists(p): r["summary"] = open(p).read().strip()
best = None
p = os.path.join(d, "best")
if os.path.exists(p): best = open(p).read().strip()
doc = {"dir": d, "stages": stages, "stopping": os.path.exists(os.path.join(d, "STOP")),
       "best": best, "rounds": [rounds[k] for k in sorted(rounds)]}
try:
    extra = json.loads(os.environ.get("FT_DETAIL") or "{}")
    if isinstance(extra, dict):
        for k, v in extra.items():
            doc.setdefault(k, v)
except ValueError:
    doc["detail_error"] = "ft_detail did not print JSON"
print(json.dumps(doc, indent=1))
EOF
    else
        echo "fleet-train: $FT_DIR"
        echo "stages: $FT_STAGES; best: $(cat "$FT_DIR/best" 2>/dev/null || echo none)$( [ -f "$FT_DIR/STOP" ] && echo '; stopping after this round')"
        for rd in "$FT_DIR"/rounds/*/; do
            [ -d "$rd" ] || continue
            printf '  %s  %s\n' "$(basename "$rd")" "$(cat "$rd/summary.txt" 2>/dev/null || echo "in hand: $(ls -t "$rd" | head -1 | sed 's/\.log$//')")"
        done
        tail -3 "$EV" 2>/dev/null | sed 's/^/  /'
    fi
    ;;

*) usage ;;
esac
