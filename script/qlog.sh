#!/usr/bin/env bash

# Renders the qlog traces in res/qlog/<library>/ as HTML pages for the
# frontend. Without arguments, renders the traces of the libraries in NQ_LIBS.

set -eu

QVIS="git+https://github.com/larseggert/qvis@3a6e0bbfe214275323746eee94ef137057f7fa91"

WORKSPACE=$(dirname "$(readlink -f "$0")")/..
cd ${WORKSPACE}/res/qlog

if [ "$#" -eq 0 ]; then
    LIBS=(${NQ_LIBS})
else
    LIBS=("$@")
fi

for LIB in "${LIBS[@]}"; do
    uvx --from ${QVIS} qvis ${LIB}/*.qlog
    # Plain scrolling over a plot should scroll the frontend: zoom only with
    # Cmd/Ctrl held.
    sed -i -e '/"wheel",/,/preventDefault/ s/if (_frozen) return;/if (_frozen || !(e.metaKey || e.ctrlKey)) return;/' \
        -e 's/Scroll to zoom/${modKey}-scroll to zoom/' ${LIB}/*.html
done
