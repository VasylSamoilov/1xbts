# SeGW: StrongSwan on Debian. Loads the rendered /etc/swanctl config on start.
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
        strongswan strongswan-swanctl libcharon-extra-plugins libstrongswan-extra-plugins \
        dnsmasq iproute2 iptables \
    && rm -rf /var/lib/apt/lists/*
COPY docker/femto/segw-entrypoint.sh /usr/local/bin/segw-entrypoint.sh
RUN chmod +x /usr/local/bin/segw-entrypoint.sh
ENTRYPOINT ["/usr/local/bin/segw-entrypoint.sh"]
