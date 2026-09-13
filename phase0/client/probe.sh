#!/bin/sh
# Sonde de phase 0 : répond aux questions 3 à 7 de CLAUDE.md §4 depuis un
# conteneur en --net host, sans relais (question 6 : worker sans port).
echo "== id"; id
echo "== env P0_*"; env | grep '^P0_' | sort
echo "== resolv.conf"; cat /etc/resolv.conf
echo "== resolv.conf : montage ?"; grep resolv /proc/mounts || echo "(pas un montage)"
echo "== resolv.conf : réécriture"
if cp /etc/resolv.conf /tmp/r && cat /tmp/r > /etc/resolv.conf; then echo "écriture ok"; else echo "écriture refusée"; fi
if rm /etc/resolv.conf && cp /tmp/r /etc/resolv.conf; then echo "remplacement ok"; else echo "remplacement refusé"; fi
echo "== interfaces"; ip -brief addr 2>/dev/null || cat /proc/net/fib_trie | grep -E "^\s+[0-9]+\.[0-9]+" | sort -u | head
while true; do
  echo "== $(date -u +%H:%M:%S)"
  for t in $P0_TARGETS; do
    echo "-- $t"
    dig +noall +comments +answer A "$t" | grep -E "status|IN" | sed 's/^/   /'
    dig +noall +comments +answer AAAA "$t" | grep -E "status|IN" | sed 's/^/   /'
    PGPASSWORD="$P0_PASSWORD" timeout 5 psql -h "$t" -U postgres -tAc "select 'pg ok via ' || inet_server_addr()" 2>&1 | head -1 | sed 's/^/   /'
    echo "   http 8080: $(curl -s -m 3 -o /dev/null -w '%{http_code}' "http://$t:8080/")"
  done
  sleep 30
done
