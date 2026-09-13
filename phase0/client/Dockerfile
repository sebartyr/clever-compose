# Sonde de phase 0 : worker sans port, piloté par le shim.
FROM postgres:16
RUN apt-get update -qq && apt-get install -y -qq --no-install-recommends dnsutils curl iproute2 >/dev/null && rm -rf /var/lib/apt/lists/*
COPY probe.sh /probe.sh
COPY cc-shim /cc-shim
COPY cc-shim.json /etc/cc-shim.json
USER root
ENTRYPOINT ["/cc-shim"]
