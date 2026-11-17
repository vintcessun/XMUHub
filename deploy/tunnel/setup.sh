#!/bin/sh
# Prepares a plain Ubuntu/Debian box to run XMUHub behind a Cloudflare Tunnel:
# swap, nginx on loopback (xmuhub-nginx.conf) and cloudflared. Run as root from this folder.
# The tunnel itself is created with scripts/cf_tunnel.py, which installs its token here.
set -e
cd "$(dirname "$0")"

# 1 GB machines: a little swap so a deploy or a burst never meets the OOM killer.
if ! swapon --show | grep -q .; then
    fallocate -l 1G /swapfile && chmod 600 /swapfile && mkswap /swapfile >/dev/null && swapon /swapfile
    grep -q '^/swapfile' /etc/fstab || echo '/swapfile none swap sw 0 0' >> /etc/fstab
    echo 'swap: 1G'
fi

export DEBIAN_FRONTEND=noninteractive
if ! command -v nginx >/dev/null; then
    apt-get update -qq && apt-get install -y -qq nginx >/dev/null
fi
# Port 80/443 may belong to something else on the box: nginx only serves loopback here.
rm -f /etc/nginx/sites-enabled/default
[ -f /etc/nginx/conf.d/default.conf ] && mv /etc/nginx/conf.d/default.conf /etc/nginx/conf.d/default.conf.disabled
mkdir -p /etc/nginx/xmuhub.d
cp -f xmuhub-nginx.conf /etc/nginx/conf.d/xmuhub.conf
nginx -t
systemctl enable --now nginx >/dev/null 2>&1
systemctl reload nginx

if ! command -v cloudflared >/dev/null; then
    mkdir -p --mode=0755 /usr/share/keyrings
    curl -fsSL https://pkg.cloudflare.com/cloudflare-main.gpg -o /usr/share/keyrings/cloudflare-main.gpg
    echo 'deb [signed-by=/usr/share/keyrings/cloudflare-main.gpg] https://pkg.cloudflare.com/cloudflared any main' > /etc/apt/sources.list.d/cloudflared.list
    apt-get update -qq && apt-get install -y -qq cloudflared >/dev/null
fi
echo "ok: nginx $(nginx -v 2>&1 | cut -d/ -f2), $(cloudflared --version | head -1)"
