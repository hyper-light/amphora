#!/usr/bin/env bash
# perf/writes units 1 and 2: clippy and the hand-off test three times, then the Records gets A/B
# against origin/main and the commit check's A/B against bdfad343. Run from the repository root.
set -uo pipefail
REPEAT=3 bash bench/writes_check.sh "--lib -- records_reads::audit_2_5_readers_finishing_together_lose_no_place" || exit 1
echo "#### records_ab get vs origin/main"
ROUNDS=3 bash bench/records_ab.sh
echo "#### records_ab commit vs bdfad343"
ROUNDS=3 OPS=commit BASE=bdfad343 FILES=crates/amphora/src/database.rs bash bench/records_ab.sh
