#!/usr/bin/env python3
"""Cloudflare cache settings for xmu.vintces.icu (see deploy/cloudflare.md).

    python3 scripts/cf_cache_rules.py          # show what the token can reach and the current state
    python3 scripts/cf_cache_rules.py apply    # make the changes below (safe to run again)

Before changing anything it saves the zone's current settings to
.secrets/cloudflare-before-<time>.json (gitignored), so they can be put back by hand.

Changes:
1. The cache rule for anonymous visitors' API calls also covers one file's details
   (/api/resources/<id>), its ratings and comments (/api/resources/<id>/social), /api/stats and
   /api/me (always {"user":null} without a cookie). The rule keeps skipping requests that carry
   the session cookie or a token, and deeper paths such as /api/resources/<id>/download (the
   mirror plan depends on the visitor's IP) are not matched.
2. /n/<id> and /r/<id> are rewritten to /n/_ and /r/_ before the cache: the server returns
   the same page whatever the id (the page reads the id from the address bar), so thousands of
   addresses share one cache entry each.
3. Smart Tiered Cache: data centres that miss ask an upper-tier one before the origin.

Needs CF_API_TOKEN in .secrets/cloudflare.env with Cache Rules, Transform Rules and
Zone Settings edit permission.
"""
import json
import os
import sys
import time
import urllib.error
import urllib.request

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ZONE = 'vintces.icu'
HOST = 'xmu.vintces.icu'
API = 'https://api.cloudflare.com/client/v4'


def read_env(path):
    env = {}
    for line in open(path, encoding='utf-8'):
        line = line.strip()
        if line and not line.startswith('#') and '=' in line:
            k, v = line.split('=', 1)
            env[k.strip()] = v.strip().strip('"')
    return env


TOKEN = read_env(os.path.join(ROOT, '.secrets', 'cloudflare.env'))['CF_API_TOKEN']


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


zone_id = call('GET', f'/zones?name={ZONE}')['result'][0]['id']
Z = f'/zones/{zone_id}'
cache = call('GET', f'{Z}/rulesets/phases/http_request_cache_settings/entrypoint')
transform = call('GET', f'{Z}/rulesets/phases/http_request_transform/entrypoint')
smart = call('GET', f'{Z}/cache/tiered_cache_smart_topology_enable')
print('cache rules readable:', cache['success'])
print('transform rules:', 'ruleset exists' if transform['success'] else transform.get('errors'))
print('smart tiered cache:', smart.get('result', {}).get('value') if smart['success'] else smart.get('errors'))

saved = os.path.join(ROOT, '.secrets', time.strftime('cloudflare-before-%Y%m%d-%H%M%S.json'))
with open(saved, 'w', encoding='utf-8') as f:
    json.dump({'cache': cache, 'transform': transform, 'smart': smart}, f, ensure_ascii=False, indent=1)
print('current settings saved to', os.path.relpath(saved, ROOT))

if sys.argv[1:] != ['apply']:
    sys.exit(0)

# 1. Anonymous visitors' API cache.
rs = cache['result']
rule = next(r for r in rs['rules'] if r.get('description', '').startswith('XMUHub public API'))
old = '(http.request.uri.path in {"/api/tree" "/api/meta" "/api/recent" "/api/popular" "/api/links" "/api/search"} or starts_with(http.request.uri.path, "/api/nodes/"))'
new = ('(http.request.uri.path in {"/api/tree" "/api/meta" "/api/recent" "/api/popular" "/api/links" "/api/search" "/api/stats" "/api/me"}'
       ' or starts_with(http.request.uri.path, "/api/nodes/")'
       ' or (http.request.uri.path wildcard "/api/resources/*" and not http.request.uri.path wildcard "/api/resources/*/*")'
       ' or http.request.uri.path wildcard "/api/resources/*/social")')
if new in rule['expression']:
    print('1. API cache rule: already done')
elif old in rule['expression']:
    body = {k: rule[k] for k in ('description', 'action', 'action_parameters', 'enabled') if k in rule}
    body['expression'] = rule['expression'].replace(old, new)
    r = call('PATCH', f"{Z}/rulesets/{rs['id']}/rules/{rule['id']}", body)
    print('1. API cache rule:', 'ok' if r['success'] else r.get('errors'))
else:
    print('1. API cache rule: its paths are not what this script expects, left alone:\n  ', rule['expression'])

# 2. One cache entry for every /n/<id>, and one for every /r/<id>.
rewrites = [{
    'description': f'XMUHub: /{k}/<id> is the same page for every id (it reads the id from the address bar): one cache entry',
    'expression': f'(http.host eq "{HOST}" and http.request.method eq "GET" and http.request.uri.path wildcard "/{k}/*")',
    'action': 'rewrite',
    'action_parameters': {'uri': {'path': {'value': f'/{k}/_'}}},
    'enabled': True,
} for k in ('n', 'r')]
if transform['success']:
    have = {x.get('description') for x in transform['result'].get('rules', [])}
    for rw in rewrites:
        if rw['description'] in have:
            print('2. rewrite: already there:', rw['expression'])
            continue
        r = call('POST', f"{Z}/rulesets/{transform['result']['id']}/rules", rw)
        print('2. rewrite:', 'ok' if r['success'] else r.get('errors'))
else:
    r = call('PUT', f'{Z}/rulesets/phases/http_request_transform/entrypoint', {'rules': rewrites})
    print('2. rewrites (new ruleset):', 'ok' if r['success'] else r.get('errors'))

# 3. Smart Tiered Cache.
r = call('PATCH', f'{Z}/cache/tiered_cache_smart_topology_enable', {'value': 'on'})
print('3. smart tiered cache:', 'on' if r['success'] else r.get('errors'))
