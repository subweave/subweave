#!/bin/sh
# Builds site/ on this machine and publishes it, with install.sh, as subweave.com on the
# Caddy proxy of steery.dev's box. Nothing builds there. The site file it replaces stays
# beside it as subweave.com.caddy.bak-<time> (the proxy imports only *.caddy), and a
# config that does not validate is put back and never loaded.
set -eu

cd "$(dirname "$0")/.."
host="${SUBWEAVE_SITE_HOST:-root@steery.dev}"
proxy=modelstat-caddy
data=/opt/modelstat/data/caddy

(cd site && npm ci --no-audit --no-fund && npm run build)
cp install.sh site/dist/install.sh

ssh "$host" "mkdir -p $data/subweave.com/site $data/subweave.com/log"
rsync -a --delete site/dist/ "$host:$data/subweave.com/site/"
scp -q deploy/subweave.com.caddy "$host:/tmp/subweave.com.caddy"
ssh "$host" "set -e
  cd $data/sites
  before=subweave.com.caddy.bak-\$(date -u +%Y%m%dT%H%M%S)
  if [ -f subweave.com.caddy ] && ! cmp -s subweave.com.caddy /tmp/subweave.com.caddy; then cp -p subweave.com.caddy \$before; fi
  install -m 644 /tmp/subweave.com.caddy subweave.com.caddy
  if ! docker exec $proxy caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile >/tmp/subweave-caddy.log 2>&1; then
    cat /tmp/subweave-caddy.log >&2
    if [ -f \$before ]; then install -m 644 \$before subweave.com.caddy; else rm -f subweave.com.caddy; fi
    echo 'the new Caddy config does not validate; the old one is back' >&2
    exit 1
  fi
  docker exec $proxy caddy reload --config /etc/caddy/Caddyfile --adapter caddyfile"
echo "published https://subweave.com"
