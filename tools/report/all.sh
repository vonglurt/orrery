#!/bin/sh
# Every lab report in docs/ as its page: REPORT SHORT-TITLE EYEBROW.
# Add a line for a new report, run it, and link the page from docs/index.html.
set -eu
cd "$(dirname "$0")/../../docs"
while IFS='|' read -r name short eyebrow; do
    [ -n "$name" ] || continue
    python3 ../tools/report/build.py "$name-lab-report.md" "$name-lab-report.html" "$short" "$eyebrow"
done <<'LIST'
appkit-gui|From Cargo to AppKit|Rust · AppKit · no crates
mac-app|Mac Applications|orrery · the app · bundles and signatures
cuda-node|A CUDA Node|orrery · the fleet · CUDA
pascal-versions|Held Back on Purpose|orrery · the fleet · CUDA versions
cuda-hybrid|One Source, Two CUDAs|orrery · crucible · CUDA 12 and 13
difficulty|Promotion Difficulty|orrery · the search · promotions
copal-machines|Copal Machines Join|orrery · the fleet · machines
storage|Fleet Storage|orrery · the fleet · storage
fleet-commanders|Fleet Commanders|orrery · the fleet · commanders
fleet-orchestration|Commanding the Fleet|orrery · the fleet · orchestration
brain-versions|Many Versions of One Brain|orrery · the brain · whole-number choices
algorhythm|Algorhythm|orrery · the brain · hybrid programming
analysis|Reading a Growing Brain|orrery · the brain · analysis
balance|Too Many Shots|orrery · the game · balance
LIST
