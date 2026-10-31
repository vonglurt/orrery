#!/bin/sh
# The machine and project buttons, against a real keeper and real jobs, with no
# fleet: every lane is this machine (target "-"), so a stopped job is really
# stopped. Each line is a claim; the first false one ends it.
#
#     sh tools/fleet-queue-check.sh
set -eu
T=$(cd "$(dirname "$0")" && pwd)
Q=$(mktemp -d)/grove
mkdir -p "$Q"
q() { python3 "$T/fleet-queue.py" "$Q" "$@"; }
trap 'q stop-now >/dev/null 2>&1 || true; sleep 1' EXIT
lane() { python3 -c "import json;print(json.load(open('$Q/lanes/$1.json'))['state'])" 2>/dev/null || echo none; }
where() { for d in queue done stopped failed; do ls "$Q/$d" | grep -q "$1" && { echo "$d"; return; }; done; ls "$Q"/running/* 2>/dev/null | grep -q "$1" && echo running || echo nowhere; }
wait_for() { # what it should say, then the command saying it
    want=$1; shift; i=0
    until [ "$("$@")" = "$want" ]; do
        i=$((i + 1)); [ $i -lt 60 ] || { echo "FAILED: $* said $("$@"), not $want"; exit 1; }
        sleep 0.5
    done
    echo "ok  $* -> $want"
}
job() { cat > "$Q/$1.in" <<E
{"id": "$1", "kind": "$2", "title": "$1", "needs": "cpu", "cmd": "$3"}
E
q add "$Q/$1.in" >/dev/null; }
cat > "$Q/config.json" <<E
{"hosts": [{"name": "a", "target": "-", "caps": ["cpu"]}, {"name": "b", "target": "-", "caps": ["cpu"]}],
 "projects": {"one": {"caps": "cpu"}, "two": {"caps": "cpu", "kinds": ["k2"]}},
 "poll_s": 1, "keep_awake": false, "sync_s": 0, "low_water": 0}
E
cat > "$Q/machines.json" <<E
{"spare": {"target": "-", "facts": {}}}
E

# Resume from a shut-down queue: the switches a stop leaves must not stop it again.
touch "$Q/SHUTDOWN" "$Q/STOP_NOW" "$Q/STOP_ADDING"
q resume >/dev/null
wait_for idle lane a
[ ! -e "$Q/STOP_ADDING" ] && echo "ok  resume takes the drain off, or the keeper would end at once"

q machine a pause >/dev/null; wait_for paused lane a
job j1 k1 "sleep 60"
wait_for running where j1
python3 -c "import json;assert json.load(open('$Q/lanes/b.json'))['job']=='j1'" && echo "ok  only the machine not paused took the job"

q machine b stop >/dev/null
wait_for queue where j1
wait_for paused lane b
[ "$(grep -c 'goes back on the queue' "$Q/logs/j1.log")" = 1 ] && echo "ok  a stopped machine's job went back, not to stopped/"

q assign a one off >/dev/null; q machine a resume >/dev/null
wait_for idle lane a
sleep 3; [ "$(where j1)" = queue ] && echo "ok  a machine taken off a project leaves that project's job on the queue"
job j2 k2 "true"
wait_for done where j2
q assign a one on >/dev/null
wait_for running where j1

q project one stop >/dev/null
wait_for queue where j1
sleep 3; [ "$(where j1)" = queue ] && echo "ok  a project stopped stays stopped"
q project one resume >/dev/null
wait_for running where j1
q stop-job j1 >/dev/null; wait_for stopped where j1

q machine a remove >/dev/null; wait_for removed lane a
python3 -c "import json;c=json.load(open('$Q/config.json'));assert [h['name'] for h in c['bench']]==['a']" && echo "ok  a removed machine waits on the bench"
q machine a add >/dev/null; wait_for idle lane a
q machine spare add >/dev/null; wait_for idle lane spare
q status --json | python3 -c "
import json,sys
r={x['name']:x for x in json.load(sys.stdin)['roster']}
assert r['spare']['joined'] and r['a']['joined'] and r['b']['paused'], r
assert r['a']['on']=={'one':True,'two':True}, r['a']" && echo "ok  the status says all of it"
echo "PASS"
