# Fixture de phase 0 : postgres:16 piloté par le shim.
# L'image officielle tourne en root et abandonne elle-même les privilèges
# (gosu dans docker-entrypoint.sh), donc pas de `user` dans cc-shim.json.
FROM postgres:16
COPY cc-shim /cc-shim
COPY cc-shim.json /etc/cc-shim.json
USER root
ENTRYPOINT ["/cc-shim"]
