#!/usr/bin/env python3
"""Cloudflare Tunnels for the site (CF_HOST in .secrets/deploy.env; see deploy/tunnel/README.md).

    python3 scripts/cf_tunnel.py status              # tunnels, their connections, where CF_HOST points
    python3 scripts/cf_tunnel.py create <name>       # create (or reuse) tunnel <name>: CF_HOST -> nginx on 127.0.0.1:8080
    python3 scripts/cf_tunnel.py install <name> <ssh-host>   # run cloudflared on <ssh-host> for tunnel <name>
    python3 scripts/cf_tunnel.py route <name>        # point CF_HOST at tunnel <name> (proxied CNAME)

Each machine gets its own tunnel, so only the one CF_HOST points at serves the site; `route`
is the switch. Before changing the DNS record it saves the old one to
.secrets/cloudflare-before-<time>.json (gitignored). The tunnel token goes straight to the
machine over ssh and is never printed.

Needs CF_API_TOKEN in .secrets/cloudflare.env with Account > Cloudflare Tunnel edit and
Zone > DNS edit permission, and CF_ACCOUNT_ID.
"""
import json
import os
import subprocess
import sys
import time
import urllib.error
import urllib.request

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
API = 'https://api.cloudflare.com/client/v4'
ORIGIN = 'http://127.0.0.1:8080'


def read_env(path):
    env = {}
    for line in open(path, encoding='utf-8'):
        line = line.strip()
        if line and not line.startswith('#') and '=' in line:
            k, v = line.split('=', 1)
            env[k.strip()] = v.strip().strip('"')
    return env


_cf = read_env(os.path.join(ROOT, '.secrets', 'cloudflare.env'))
TOKEN, ACCOUNT = _cf['CF_API_TOKEN'], _cf['CF_ACCOUNT_ID']
_deploy = read_env(os.path.join(ROOT, '.secrets', 'deploy.env'))
ZONE, HOST = _deploy['CF_ZONE'], _deploy['CF_HOST']
A = f'/accounts/{ACCOUNT}/cfd_tunnel'


def call(method, path, body=None):
    last = None
    for _ in range(4):
        try:
            req = urllib.request.Request(API + path, method=method, data=None if body is None else json.dumps(body).encode(),
                                         headers={'Authorization': 'Bearer ' + TOKEN, 'Content-Type': 'application/json'})
            with urllib.request.urlopen(req, timeout=30) as r:
                return json.load(r)
        except urllib.error.HTTPError as e:
            try:
                return json.load(e)
            except ValueError:
                return {'success': False, 'errors': [{'message': f'HTTP {e.code}'}]}
        except OSError as e:  # network hiccup: try again
            last = e
            time.sleep(2)
    return {'success': False, 'errors': [{'message': str(last)}]}


def ok(res, what):
    if not res.get('success'):
        sys.exit(f'{what} failed: {res.get("errors")}')
    return res['result']


def tunnel(name):
    found = ok(call('GET', f'{A}?name={name}&is_deleted=false'), 'list tunnels')
    return found[0] if found else None


def zone_id():
    return ok(call('GET', f'/zones?name={ZONE}'), 'find zone')[0]['id']


def record():
    recs = ok(call('GET', f'/zones/{zone_id()}/dns_records?name={HOST}'), 'read DNS')
    return recs[0] if recs else None


def status():
    for t in ok(call('GET', f'{A}?is_deleted=false'), 'list tunnels'):
        conns = ok(call('GET', f'{A}/{t["id"]}/connections'), 'connections')
        where = sorted({c.get('origin_ip', '?') for c in conns})
        print(f'{t["name"]}: {t["status"]}, {sum(len(c.get("conns", [])) for c in conns)} connections from {where or "-"}  ({t["id"]}.cfargotunnel.com)')
    r = record()
    print(f'{HOST}: {r["type"]} {r["content"]} {"proxied" if r["proxied"] else "dns-only"}' if r else f'{HOST}: no record')


def create(name):
    t = tunnel(name) or ok(call('POST', A, {'name': name, 'config_src': 'cloudflare'}), 'create tunnel')
    # Other host names the machine serves through the same tunnel (added by hand) are kept.
    current = call('GET', f'{A}/{t["id"]}/configurations')
    rules = ((current.get('result') or {}).get('config') or {}).get('ingress') or []
    others = [r for r in rules if r.get('hostname') and r['hostname'] != HOST]
    ingress = {'config': {'ingress': [{'hostname': HOST, 'service': ORIGIN}, *others, {'service': 'http_status:404'}]}}
    ok(call('PUT', f'{A}/{t["id"]}/configurations', ingress), 'set ingress')
    print(f'tunnel {name}: {t["id"]} ({HOST} -> {ORIGIN})')


def install(name, ssh_host):
    t = tunnel(name) or sys.exit(f'no tunnel {name}; run create first')
    token = ok(call('GET', f'{A}/{t["id"]}/token'), 'tunnel token')
    # Token on stdin, never on a command line or in the output.
    script = 'set -e; read -r T; cloudflared service uninstall >/dev/null 2>&1 || true; cloudflared service install "$T" >/dev/null 2>&1; systemctl enable --now cloudflared >/dev/null 2>&1; sleep 3; systemctl is-active cloudflared'
    subprocess.run(['ssh', ssh_host, script], input=token + '\n', text=True, check=True)


def route(name):
    t = tunnel(name) or sys.exit(f'no tunnel {name}; run create first')
    target = f'{t["id"]}.cfargotunnel.com'
    z, r = zone_id(), record()
    if r and r['type'] == 'CNAME' and r['content'] == target and r['proxied']:
        print(f'{HOST} already points at {name}')
        return
    saved = os.path.join(ROOT, '.secrets', time.strftime('cloudflare-before-%Y%m%d-%H%M%S.json'))
    with open(saved, 'w', encoding='utf-8') as f:
        json.dump({'dns': r}, f, ensure_ascii=False, indent=1)
    body = {'type': 'CNAME', 'name': HOST, 'content': target, 'proxied': True, 'ttl': 1}
    if r:
        ok(call('PUT', f'/zones/{z}/dns_records/{r["id"]}', body), 'update DNS')
    else:
        ok(call('POST', f'/zones/{z}/dns_records', body), 'create DNS')
    print(f'{HOST} -> {name} (old record saved to {os.path.relpath(saved, ROOT)})')


if __name__ == '__main__':
    cmd, args = (sys.argv[1], sys.argv[2:]) if len(sys.argv) > 1 else ('status', [])
    {'status': status, 'create': create, 'install': install, 'route': route}[cmd](*args)
