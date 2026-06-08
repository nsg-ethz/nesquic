#!/bin/bash
set -e

PREFIX=()

if [ -n "${MM_DELAY}" ] && [ "${MM_DELAY}" != "0" ]; then
    PREFIX+=(mm-delay "${MM_DELAY}")
fi

if [ -n "${MM_LOSS}" ] && [ "${MM_LOSS}" != "0" ]; then
    PREFIX+=(mm-loss uplink "${MM_LOSS}")
fi

if [ -n "${MM_LINK}" ]; then
    PREFIX+=(mm-link "/workspace/res/traces/${MM_LINK}.up" "/workspace/res/traces/${MM_LINK}.down" --)
fi

exec "${PREFIX[@]}" "${NESQUIC_BIN}" "$@"
