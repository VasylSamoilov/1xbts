#!/bin/sh
# Run on the Docker host (needs root) after the stack is up. Adds the host
# return route to the FAP VIP pool via the SeGW — Compose sets up container
# networking but not host routes. Re-run after recreating the SeGW container.
#
# Usage: host-route.sh [add|del]
set -e

VIP_POOL="${FEMTO_VIP_POOL:-10.55.100.0/24}"
SERVICE_IP="${FEMTO_SERVICE_IP:-10.56.0.2}"
SEGW_IP="${FEMTO_SEGW_EDGE_IP:-172.31.0.2}"
PDSN_IP="${FEMTO_PDSN_EDGE_IP:-172.31.0.1}"
EDGE_SUBNET="${FEMTO_EDGE_SUBNET:-172.31.0.0/24}"
ACTION="${1:-add}"

edge_interface() {
    ip route show "$EDGE_SUBNET" | awk '/ dev / { for (i = 1; i <= NF; i++) if ($i == "dev") { print $(i + 1); exit } }'
}

add_input_rule() {
    if ! iptables -C INPUT "$@" -j ACCEPT 2>/dev/null; then
        iptables -I INPUT 1 "$@" -j ACCEPT
    fi
}

del_input_rule() {
    while iptables -C INPUT "$@" -j ACCEPT 2>/dev/null; do
        iptables -D INPUT "$@" -j ACCEPT
    done
}

case "$ACTION" in
    add)
        if ip route show "$VIP_POOL" | grep -q "via $SEGW_IP"; then
            echo "[host-route] $VIP_POOL already routed via $SEGW_IP"
        else
            ip route replace "$VIP_POOL" via "$SEGW_IP"
            echo "[host-route] $VIP_POOL -> $SEGW_IP"
        fi
        ip route replace "$SERVICE_IP/32" via "$SEGW_IP"
        echo "[host-route] $SERVICE_IP/32 -> $SEGW_IP"
        EDGE_IF="$(edge_interface)"
        if [ -z "$EDGE_IF" ]; then
            echo "[host-route] no interface found for $EDGE_SUBNET" >&2
            exit 1
        fi
        add_input_rule -i "$EDGE_IF" -s "$VIP_POOL" -d "$PDSN_IP" -p udp --dport 699
        add_input_rule -i "$EDGE_IF" -s "$VIP_POOL" -d "$PDSN_IP" -p 47
        add_input_rule -i "$EDGE_IF" -s "$VIP_POOL" -d "$PDSN_IP" -p udp --dport 1812
        echo "[host-route] allowed A11, GRE, and A12 from $VIP_POOL to $PDSN_IP on $EDGE_IF"
        ;;
    del)
        EDGE_IF="$(edge_interface)"
        if [ -n "$EDGE_IF" ]; then
            del_input_rule -i "$EDGE_IF" -s "$VIP_POOL" -d "$PDSN_IP" -p udp --dport 699
            del_input_rule -i "$EDGE_IF" -s "$VIP_POOL" -d "$PDSN_IP" -p 47
            del_input_rule -i "$EDGE_IF" -s "$VIP_POOL" -d "$PDSN_IP" -p udp --dport 1812
        fi
        ip route del "$SERVICE_IP/32" via "$SEGW_IP" 2>/dev/null || true
        ip route del "$VIP_POOL" via "$SEGW_IP" 2>/dev/null || true
        echo "[host-route] removed femtocell routes and PDSN input rules"
        ;;
    *)
        echo "usage: $0 [add|del]" >&2
        exit 2
        ;;
esac
