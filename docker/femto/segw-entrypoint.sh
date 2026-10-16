#!/bin/sh
# Start charon, load the mounted swanctl config, then stay in the foreground.
set -e

FEMTO_SERVICE_IP="${FEMTO_SERVICE_IP:-10.56.0.2}"
FEMTO_DNS_IP="${FEMTO_DNS_IP:-10.56.0.2}"
FEMTO_DNS_DOMAIN="${FEMTO_DNS_DOMAIN:-femto.local.1xbts.org}"
FEMTO_ACS_DOMAIN="${FEMTO_ACS_DOMAIN:-fsmhdm.femto.spcsdns.net}"
FEMTO_SIP_DOMAIN="${FEMTO_SIP_DOMAIN:-fap.airave2.ims.spcsdns.net}"
ip address replace "$FEMTO_SERVICE_IP/32" dev lo
ip address replace "$FEMTO_DNS_IP/32" dev lo

dnsmasq \
    --keep-in-foreground \
    --listen-address="$FEMTO_DNS_IP" \
    --bind-interfaces \
    --address="/$FEMTO_DNS_DOMAIN/$FEMTO_SERVICE_IP" \
    --address="/$FEMTO_ACS_DOMAIN/$FEMTO_SERVICE_IP" \
    --address="/$FEMTO_SIP_DOMAIN/$FEMTO_SERVICE_IP" &
DNSMASQ_PID=$!

echo "[segw] starting charon"
/usr/lib/ipsec/charon &
CHARON_PID=$!

# Wait for the vici socket charon exposes for swanctl.
i=0
while [ ! -S /var/run/charon.vici ] && [ "$i" -lt 30 ]; do
    sleep 1
    i=$((i + 1))
done

monitor_config() {
    CONFIG_PATH=/etc/swanctl/swanctl.conf
    CONFIG_HASH=

    while kill -0 "$CHARON_PID" 2>/dev/null; do
        if [ -f "$CONFIG_PATH" ]; then
            NEW_CONFIG_HASH=$(sha256sum "$CONFIG_PATH" | cut -d ' ' -f 1)
            if [ "$NEW_CONFIG_HASH" != "$CONFIG_HASH" ]; then
                echo "[segw] loading $CONFIG_PATH"
                if swanctl --load-all; then
                    CONFIG_HASH=$NEW_CONFIG_HASH
                    echo "[segw] loaded connections:"
                    swanctl --list-conns || true
                else
                    echo "[segw] swanctl --load-all reported errors"
                fi
            fi
        fi
        sleep 1
    done
}

monitor_config &
MONITOR_PID=$!

set +e
wait "$CHARON_PID"
CHARON_STATUS=$?
set -e
kill "$MONITOR_PID" 2>/dev/null || true
wait "$MONITOR_PID" 2>/dev/null || true
kill "$DNSMASQ_PID" 2>/dev/null || true
wait "$DNSMASQ_PID" 2>/dev/null || true
exit "$CHARON_STATUS"
