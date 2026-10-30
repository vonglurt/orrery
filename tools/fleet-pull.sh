#!/bin/sh
# Pull every project on a machine: each git checkout in the user's home
# (~/code, ~/dev, and the home itself, three levels down at most), brought
# up to its upstream. Paul, 29 October: "a git pull of all the projects in
# vonglurt and the others too like treadcore."
#
# Run as the user, no root: orrery's Machines view runs it over ssh
# (machines.py pull NAME) and shows a line a checkout.
#
# Only a fast-forward: a checkout with changes of its own, on a branch with
# no upstream, or one that has gone its own way is left as it is and said
# so. Nothing is merged, rebased, stashed or reset.
set -u

found=0
for g in $(find "$HOME" -maxdepth 4 -type d -name .git -not -path '*/.cache/*' -not -path '*/target/*' -not -path '*/fleet/*' 2>/dev/null | sort); do
    d=${g%/.git}
    name=${d#$HOME/}
    found=$((found + 1))
    if ! git -C "$d" rev-parse --abbrev-ref --symbolic-full-name '@{u}' >/dev/null 2>&1; then
        echo "$name: no upstream, left as it is"
        continue
    fi
    if ! git -C "$d" diff --quiet 2>/dev/null || ! git -C "$d" diff --cached --quiet 2>/dev/null; then
        echo "$name: changes of its own, left as it is"
        continue
    fi
    old=$(git -C "$d" rev-parse --short HEAD)
    if out=$(git -C "$d" pull -q --ff-only 2>&1); then
        new=$(git -C "$d" rev-parse --short HEAD)
        if [ "$old" = "$new" ]; then
            echo "$name: up to date ($new)"
        else
            echo "$name: $old -> $new"
        fi
    else
        echo "$name: not a fast-forward, left as it is ($(echo "$out" | head -1))"
    fi
done
[ "$found" -gt 0 ] || echo "no git checkouts in $HOME"
