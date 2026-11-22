#!/usr/bin/env bash
# Build the arXiv submission tarball at paper/build/arxiv.tar.gz.
#
#   scripts/arxiv-bundle.sh            rebuild the PDF from committed results, then bundle
#   scripts/arxiv-bundle.sh --no-build bundle the current build as is
#   scripts/arxiv-bundle.sh --check    also compile the tarball in a fresh
#                                      temporary directory with pdflatex alone
#
# The bundle holds what arXiv needs to compile the paper, at the paths
# main.tex expects:
#   main.tex, main.bbl (arXiv does not run BibTeX), sections/*.tex,
#   results/numbers.tex (the generated \R{} macros), results/tables/*.tex
#   (the generated tables), and the figures main.tex includes.
# No build artifacts, result CSVs, bibliography source, figure sources or
# absolute paths. Every package used is in TeX Live, so no style files are
# shipped.
set -euo pipefail
# No macOS AppleDouble (._*) entries in the tarball.
export COPYFILE_DISABLE=1
cd "$(dirname "$0")/.."

build=1
check=0
for a in "$@"; do
  case "$a" in
    --no-build) build=0 ;;
    --check) check=1 ;;
    *) echo "unknown option: $a" >&2; exit 2 ;;
  esac
done

if [ "$build" = 1 ]; then
  scripts/paper.sh --pdf-only
fi

P=paper
BBL=$P/build/latex/main.bbl
[ -f "$BBL" ] || { echo "missing $BBL: run scripts/paper.sh --pdf-only" >&2; exit 1; }

STAGE=$(mktemp -d)
trap 'rm -rf "$STAGE"' EXIT

mkdir -p "$STAGE/sections" "$STAGE/results/tables" "$STAGE/figures"
cp $P/main.tex "$STAGE/"
cp "$BBL" "$STAGE/main.bbl"
cp $P/sections/*.tex "$STAGE/sections/"
cp $P/results/numbers.tex "$STAGE/results/"
cp $P/results/tables/*.tex "$STAGE/results/tables/"

# Exactly the figures the sources include.
figs=$(grep -rhoE '\\includegraphics(\[[^]]*\])?\{[^}]+\}' $P/main.tex $P/sections \
  | sed -E 's/.*\{([^}]*)\}$/\1/' | sort -u)
for f in $figs; do
  src=$P/$f
  [ -f "$src" ] || { echo "missing figure: $src" >&2; exit 1; }
  cp "$src" "$STAGE/$f"
done

# No absolute paths may leak into the bundle.
if grep -rlE '/Users/|/home/|/private/|/tmp/|[A-Z]:\\' "$STAGE" --include='*.tex' --include='*.bbl'; then
  echo "absolute path found in the files above" >&2
  exit 1
fi

OUT=$P/build/arxiv.tar.gz
# Deterministic order and ownership.
tar -C "$STAGE" -czf "$OUT" --uid 0 --gid 0 --uname "" --gname "" \
  main.tex main.bbl sections results figures 2>/dev/null \
  || tar -C "$STAGE" -czf "$OUT" main.tex main.bbl sections results figures
echo "wrote $OUT ($(du -h "$OUT" | cut -f1), $(tar -tzf "$OUT" | grep -vc '/$') files)"

if [ "$check" = 1 ]; then
  T=$(mktemp -d)
  trap 'rm -rf "$STAGE" "$T"' EXIT
  tar -xzf "$OUT" -C "$T"
  # pdflatex alone, three passes so cross-references settle (as arXiv
  # reruns until stable); no BibTeX, no scripts.
  for i in 1 2 3; do
    (cd "$T" && pdflatex -interaction=nonstopmode -halt-on-error main.tex >/dev/null) \
      || { echo "pdflatex failed; log: $T/main.log" >&2; trap - EXIT; exit 1; }
  done
  pages=$(grep -oE 'Output written on main.pdf \(([0-9]+) pages' "$T/main.log" | grep -oE '[0-9]+ pages')
  warn=$(grep -cE 'LaTeX Warning|Citation .* undefined|Reference .* undefined|Overfull|Underfull' "$T/main.log" || true)
  echo "check: compiled in a fresh directory with pdflatex alone: $pages, $warn warnings"
fi
