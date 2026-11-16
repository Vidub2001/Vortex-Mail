#!/usr/bin/env bash
# Lance les cibles de fuzzing en parallèle (lot 8 de docs/plan-lots.md).
#
#   scripts/fuzz.sh [secondes par cible] [cible…]
#   scripts/fuzz.sh 60                 essai rapide, toutes les cibles
#   scripts/fuzz.sh 28800              une nuit : 8 h par cible, en parallèle
#   scripts/fuzz.sh 600 ical vcard     seulement ces cibles
#
# Il faut Rust nightly et cargo-fuzz (voir fuzz/README.md).
# SANITIZER=none scripts/fuzz.sh …   sans AddressSanitizer (plus rapide).
# Journaux : target/fuzz-logs/<cible>.log ; entrées qui plantent :
# fuzz/artifacts/<cible>/.
set -euo pipefail
cd "$(dirname "$0")/.."

DUREE="${1:-60}"
shift || true
SANITIZER="${SANITIZER:-address}"
CIBLES=("$@")
if [ ${#CIBLES[@]} -eq 0 ]; then
    mapfile -t CIBLES < <(cargo +nightly fuzz list)
fi

# Compilation une seule fois, avec les vérifications de débordement (-a).
cargo +nightly fuzz build -a -s "$SANITIZER"

mkdir -p target/fuzz-logs
declare -A PIDS
for cible in "${CIBLES[@]}"; do
    amorces="fuzz/seeds/$cible"
    # Les messages : les mails piégés du corpus de non-régression.
    [ "$cible" = mime ] && amorces="tests/corpus/attaques"
    # Entrées qui ont déjà fait planter cette cible : rejouées à chaque fois.
    regressions=()
    [ -d "tests/corpus/fuzz/$cible" ] && regressions=("tests/corpus/fuzz/$cible")
    mkdir -p "fuzz/corpus/$cible"
    cargo +nightly fuzz run -a -s "$SANITIZER" "$cible" "fuzz/corpus/$cible" "$amorces" "${regressions[@]}" -- \
        -max_total_time="$DUREE" -timeout=10 -rss_limit_mb=4096 -print_final_stats=1 \
        > "target/fuzz-logs/$cible.log" 2>&1 &
    PIDS[$cible]=$!
    echo "… $cible (journal : target/fuzz-logs/$cible.log)"
done

echecs=0
for cible in "${!PIDS[@]}"; do
    if wait "${PIDS[$cible]}"; then
        execs=$(grep -o 'stat::number_of_executed_units: *[0-9]*' "target/fuzz-logs/$cible.log" | grep -o '[0-9]*$' || echo "?")
        echo "✔ $cible : aucun plantage ($execs entrées essayées)"
    else
        echecs=$((echecs + 1))
        echo "✘ $cible : plantage, voir target/fuzz-logs/$cible.log et fuzz/artifacts/$cible/"
    fi
done
exit "$echecs"
