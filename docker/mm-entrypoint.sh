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

if [ ${#PREFIX[@]} -eq 0 ]; then
    exec "${NESQUIC_BIN}" "$@"
fi

# Expand a literal $MAHIMAHI_BASE in the arguments and INFLUX_URL inside the
# emulated network, where mahimahi has set it to the host-side address.
exec "${PREFIX[@]}" bash -c 'INFLUX_URL="${INFLUX_URL//\$MAHIMAHI_BASE/$MAHIMAHI_BASE}" exec "$0" "${@//\$MAHIMAHI_BASE/$MAHIMAHI_BASE}"' "${NESQUIC_BIN}" "$@"
