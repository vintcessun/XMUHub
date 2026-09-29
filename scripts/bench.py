#!/usr/bin/env python3
"""Performance benchmark for a LOCAL XMUHub server (never production).

    # 1. a throwaway server with local storage and a high rate limit
    XMUHUB_DATA=/tmp/bench XMUHUB_STORAGE=local XMUHUB_SCRIPT_TOKEN=<32+ chars> \\
      XMUHUB_RATE_LIMIT=100000 XMUHUB_SECURE_COOKIE=0 cargo run --release -p xmuhub-server
    # 2. fill it with production-sized fake data (10 colleges, 200 courses, 3000 files)
    python3 scripts/bench.py seed --token <same token>
    # 3. measure
    python3 scripts/bench.py run --token <same token> [--json report.json]

`run` times every endpoint one request at a time (median / p90 / p99 over --n requests),
then throughput with --threads clients reading at once, then the same reads while a batch
upload is going on (the case where writes used to hold readers up).

Only talks to 127.0.0.1 / localhost unless --allow-remote is given.
"""
import argparse
import concurrent.futures as cf
import hashlib
import http.client
import json
import random
import statistics
import sys
import threading
import time
import urllib.error
import urllib.parse
import urllib.request

P = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
P.add_argument('mode', choices=['seed', 'run'])
P.add_argument('--base', default='http://127.0.0.1:8089')
P.add_argument('--token', required=True, help='XMUHUB_SCRIPT_TOKEN of the local server')
P.add_argument('--n', type=int, default=200, help='requests per endpoint in the latency pass')
P.add_argument('--threads', type=int, default=16)
P.add_argument('--seconds', type=float, default=8)
P.add_argument('--json', help='also write the results here')
P.add_argument('--allow-remote', action='store_true')
P.add_argument('--scale', type=int, default=1, help='seed: multiply courses and files (1 = production size)')
A = P.parse_args()
if urllib.parse.urlparse(A.base).hostname not in ('127.0.0.1', 'localhost', '::1') and not A.allow_remote:
    sys.exit('refusing to benchmark a non-local server (use --allow-remote if you really mean it)')
BASE = A.base.rstrip('/')
AUTH = {'Authorization': 'Bearer ' + A.token}


_local = threading.local()
_U = urllib.parse.urlparse(BASE)


def req(method, path, body=None, auth=True, raw=None, headers=None):
    """One request on this thread's kept-alive connection: what is timed is the server, not
    TCP setup (a new connection costs ~5 ms on its own on Windows)."""
    h = {**(AUTH if auth else {}), **(headers or {})}
    data = raw
    if body is not None:
        data = json.dumps(body).encode()
        h['Content-Type'] = 'application/json'
    if path.startswith('http'):
        path = urllib.parse.urlparse(path)._replace(scheme='', netloc='').geturl()
    for attempt in range(2):
        c = getattr(_local, 'conn', None)
        if c is None:
            c = _local.conn = http.client.HTTPConnection(_U.hostname, _U.port or 80, timeout=60)
        try:
            c.request(method, path, body=data, headers=h)
            resp = c.getresponse()
            return resp.status, resp.read()
        except (http.client.HTTPException, OSError):
            c.close()
            _local.conn = None
            if attempt:
                raise


def api(method, path, body=None, auth=True):
    status, out = req(method, '/api' + path, body, auth)
    if status >= 400:
        raise RuntimeError(f'{method} {path} -> {status} {out[:200]!r}')
    return json.loads(out) if out else None


# ---------------------------------------------------------------- seed

COLLEGES = ['信息学院', '数学科学学院', '物理科学与技术学院', '化学化工学院', '经济学院', '管理学院', '外文学院', '人文学院', '医学院', '生命科学学院']
WORDS = ['原理', '导论', '基础', '分析', '设计', '实验', '方法', '概论', '专题', '前沿']
STEMS = ['数据', '信号', '电路', '算法', '网络', '材料', '有机', '宏观', '微观', '统计', '代数', '几何', '力学', '光学', '生理', '语言', '文学', '历史', '会计', '金融']
TYPES = ['期末试卷', '期中试卷', '提纲', '课件', '讲义', '题库']


def upload(node, name, content, extra=None):
    sha = hashlib.sha256(content).hexdigest()
    plan = api('POST', '/uploads', {'filename': name, 'mime': 'application/pdf', 'parts': [{'size': len(content), 'sha256': sha}]})
    if not plan.get('dedup'):
        t = plan['parts'][0]['target']
        status, out = req(t['method'], t['url'], raw=content, auth=False, headers={**dict(t['headers']), 'X-XMUHub': '1'})
        if status >= 400:
            raise RuntimeError(f'part upload -> {status} {out[:200]!r}')
        api('POST', f"/uploads/{plan['upload_id']}/parts/0", {'asset_id': None})
    body = {'upload_id': plan['upload_id'], 'node': node, 'time': str(random.randint(2012, 2025)), 'type_word': random.choice(TYPES), 'extra': name[:6]}
    if extra:
        body['admin'] = extra
    return api('POST', '/resources', body)


def seed():
    random.seed(1)
    have = {n['name']: n for n in api('GET', '/tree')}
    sec = have.get('专业课(压测)') or api('POST', '/nodes', {'parent': None, 'kind': 'section', 'name': '专业课(压测)'})
    courses = []
    for c in COLLEGES:
        g = api('POST', '/nodes', {'parent': sec['id'], 'kind': 'group', 'name': c + '(压测)'})
        for k in range(20 * A.scale):
            courses.append(api('POST', '/nodes', {'parent': g['id'], 'kind': 'course', 'name': random.choice(STEMS) + random.choice(WORDS) + f'{k}'})['id'])
    print(f'{len(courses)} courses')
    t0 = time.time()
    jobs = [(c, f'{i}_{j}.pdf', f'bench file {c} {i} {j} '.encode() * 40) for i, c in enumerate(courses) for j in range(15)]
    done = [0]

    def one(job):
        r = upload(*job)
        done[0] += 1
        if done[0] % 500 == 0:
            print(f'  {done[0]} files, {time.time() - t0:.0f} s', flush=True)
        return r['id']

    with cf.ThreadPoolExecutor(8) as ex:
        ids = list(ex.map(one, jobs))
    # Comments are rate-limited per account (a few seconds apart), so only ratings here.
    for rid in random.sample(ids, 300):
        api('PUT', f'/resources/{rid}/rating', {'stars': random.randint(3, 5)})
    # The rest of the site: 求资料, 合集, shared 收藏夹, follows (the import account is exempt
    # from review, so all of them are live at once).
    for i in range(60 * A.scale):
        code, out = req('POST', '/api/wants', {'title': f'求{random.choice(STEMS)}{random.choice(WORDS)}往年卷 {i}', 'body': '有的同学麻烦传一下', 'node': random.choice(courses)})
        if code == 429:
            print(f'  求资料: daily cap after {i}')
            break
        wt = json.loads(out)

        if i >= 4:  # an account may have 5 open posts at a time
            api('POST', f"/wants/{wt['id']}/status", {'status': random.choice(['found', 'closed'])})
    by_course = {}
    for rid, (c, _, _) in zip(ids, jobs):
        by_course.setdefault(c, []).append(rid)
    for c in random.sample(courses, min(len(courses), 40 * A.scale)):
        api('POST', '/series', {'node': c, 'title': '历年期末合集', 'source': '压测', 'year': '2012–2025', 'items': by_course[c][:8]})
    for i in range(15):  # up to 20 per account; leave room for the run
        col = api('POST', '/collections', {'title': f'复习必备 {i}', 'note': ''})
        for rid in random.sample(ids, 30):
            api('PUT', f"/collections/{col['id']}/items/{rid}", {'on': True})
        api('POST', f"/collections/{col['id']}/share", {'on': True})
    for c in random.sample(courses, 50):
        api('PUT', f'/nodes/{c}/follow', {'on': True})
    print(f'{len(ids)} files in {time.time() - t0:.0f} s; ratings, wants, 合集, 收藏夹, follows added')


# ---------------------------------------------------------------- run

def pct(v, p):
    v = sorted(v)
    return v[min(len(v) - 1, int(round(p / 100 * (len(v) - 1))))]


def timed(fn):
    t = time.perf_counter()
    fn()
    return (time.perf_counter() - t) * 1000


def run():
    tree = api('GET', '/tree')
    courses = [n for n in tree if n['kind'] == 'course' and n['count']]
    groups = [n for n in tree if n['kind'] == 'group']
    if len(courses) < 50:
        sys.exit('not enough data: run "bench.py seed" first')
    # Reads: a course of median size. Writes go to their own course, so repeated runs don't
    # grow the one being read.
    scratch = next((n for n in tree if n['name'] == '压测写入'), None) or api('POST', '/nodes', {'parent': groups[1]['id'], 'kind': 'course', 'name': '压测写入'})
    courses = [n for n in courses if n['id'] != scratch['id']]
    course = sorted(courses, key=lambda n: n['count'])[len(courses) // 2]
    biggest = max(courses, key=lambda n: n['count'])
    res = api('GET', f"/nodes/{course['id']}")['resources']
    rid = res[0]['id']
    rnd = random.Random(2)
    pick_res = lambda: rnd.choice(res)['id']
    wants = api('GET', '/wants')
    want_id = wants[0]['id'] if wants else 0
    cols = api('GET', '/collections?scope=public')
    col_id = cols[0]['id'] if cols else 0
    series_id = next((sr['id'] for n in courses[:200] for sr in (api('GET', f"/nodes/{n['id']}").get('series') or [])), 0)
    q = lambda s, t='course', extra='': f'/search?q={urllib.parse.quote(s)}&type={t}{extra}'

    reads = [
        ('GET /api/meta', lambda: req('GET', '/api/meta', auth=False)),
        ('GET /api/me (guest)', lambda: req('GET', '/api/me', auth=False)),
        ('GET /api/me (token)', lambda: req('GET', '/api/me')),
        ('GET /api/tree', lambda: req('GET', '/api/tree', auth=False)),
        ('GET /api/recent', lambda: req('GET', '/api/recent', auth=False)),
        ('GET /api/popular', lambda: req('GET', '/api/popular', auth=False)),
        ('GET /api/stats', lambda: req('GET', '/api/stats', auth=False)),
        ('search course 中文', lambda: req('GET', '/api' + q('数据'), auth=False)),
        ('search course 拼音首字母', lambda: req('GET', '/api' + q('sjfx'), auth=False)),
        ('search resource', lambda: req('GET', '/api' + q('期末', 'resource'), auth=False)),
        ('search resource p5', lambda: req('GET', '/api' + q('试卷', 'resource', '&page=5'), auth=False)),
        ('search within college', lambda: req('GET', '/api' + q('原理', 'course', f"&within={groups[0]['id']}"), auth=False)),
        ('search 64 chars', lambda: req('GET', '/api' + q('数据结构算法分析' * 8), auth=False)),
        ('GET /api/nodes/<course>', lambda: req('GET', f"/api/nodes/{course['id']}", auth=False)),
        (f"GET /api/nodes/<biggest, {biggest['count']} files>", lambda: req('GET', f"/api/nodes/{biggest['id']}", auth=False)),
        ('GET /api/nodes/<college>', lambda: req('GET', f"/api/nodes/{groups[0]['id']}", auth=False)),
        ('GET /api/resources/<id>', lambda: req('GET', f'/api/resources/{pick_res()}', auth=False)),
        ('GET /api/resources/<id>/social', lambda: req('GET', f'/api/resources/{pick_res()}/social', auth=False)),
        ('GET /api/resources/<id>/download', lambda: req('GET', f'/api/resources/{pick_res()}/download?peek=1', auth=False)),
        ('GET /api/review (staff)', lambda: req('GET', '/api/review')),
        ('GET /api/admin/users (staff)', lambda: req('GET', '/api/admin/users?q=')),
        ('GET /api/nodes/suggest', lambda: req('GET', '/api/nodes/suggest?q=' + urllib.parse.quote('数据'), auth=False)),
        ('GET /api/resources/<id>/reviews', lambda: req('GET', f'/api/resources/{pick_res()}/reviews')),
        ('GET /api/resources/<id>/collections', lambda: req('GET', f'/api/resources/{pick_res()}/collections')),
        ('GET /api/mine', lambda: req('GET', '/api/mine?limit=40')),
        ('GET /api/me/questions', lambda: req('GET', '/api/me/questions')),
        ('GET /api/me/follows', lambda: req('GET', '/api/me/follows')),
        ('GET /api/me/tokens', lambda: req('GET', '/api/me/tokens')),
        ('GET /api/feedback/mine', lambda: req('GET', '/api/feedback/mine')),
        ('GET /api/notices', lambda: req('GET', '/api/notices')),
        ('GET /api/nodes/<id>/follow', lambda: req('GET', f"/api/nodes/{course['id']}/follow")),
        ('GET /api/links', lambda: req('GET', '/api/links', auth=False)),
        ('GET /api/links/suggestions (staff)', lambda: req('GET', '/api/links/suggestions')),
        ('GET /api/wants', lambda: req('GET', '/api/wants', auth=False)),
        ('GET /api/wants?node=<course>', lambda: req('GET', f"/api/wants?node={course['id']}", auth=False)),
        ('GET /api/wants/<id>', lambda: req('GET', f'/api/wants/{want_id}', auth=False)),
        ('GET /api/collections?scope=public', lambda: req('GET', '/api/collections?scope=public', auth=False)),
        ('GET /api/collections (mine)', lambda: req('GET', '/api/collections')),
        ('GET /api/collections/<id>', lambda: req('GET', f'/api/collections/{col_id}', auth=False)),
        ('GET /api/series/<id>', lambda: req('GET', f'/api/series/{series_id}', auth=False)),
        ('GET /api/review/change-requests (staff)', lambda: req('GET', '/api/review/change-requests')),
        ('GET /api/review/nodes (staff)', lambda: req('GET', '/api/review/nodes')),
        ('GET /api/review/batch (staff)', lambda: (req('GET', '/api/review/batch'), req('POST', '/api/review/batch/release', headers={'X-XMUHub': '1'}))),
        ('GET /api/review/collections (staff)', lambda: req('GET', '/api/review/collections')),
        ('GET /api/review/series (staff)', lambda: req('GET', '/api/review/series')),
        ('GET /api/review?status=published (staff)', lambda: req('GET', '/api/review?status=published')),
        ('GET /api/admin/avatars (staff)', lambda: req('GET', '/api/admin/avatars')),
        ('GET /api/admin/reports (staff)', lambda: req('GET', '/api/admin/reports')),
        ('GET /api/admin/feedback (staff)', lambda: req('GET', '/api/admin/feedback')),
        ('GET /api/admin/reviews (staff)', lambda: req('GET', '/api/admin/reviews')),
        ('GET /api/admin/daily (staff)', lambda: req('GET', '/api/admin/daily')),
        ('GET /api/admin/status (staff)', lambda: req('GET', '/api/admin/status')),
        ('GET /api/admin/thumbs (staff)', lambda: req('GET', '/api/admin/thumbs')),
        ('GET /d/<id> (redirect)', lambda: req('GET', f'/d/{pick_res()}', auth=False)),
        ('POST /mcp search', lambda: req('POST', '/mcp', {'jsonrpc': '2.0', 'id': 1, 'method': 'tools/call', 'params': {'name': 'search', 'arguments': {'q': '期末'}}})),
        ('POST /mcp get_category', lambda: req('POST', '/mcp', {'jsonrpc': '2.0', 'id': 1, 'method': 'tools/call', 'params': {'name': 'get_category', 'arguments': {'id': course['id']}}})),
        ('page /', lambda: req('GET', '/', auth=False)),
        ('page /n/<id>', lambda: req('GET', f"/n/{course['id']}", auth=False)),
        ('static app.js', lambda: req('GET', '/assets/app.js', auth=False)),
    ]
    seq = [0]

    def next_name():
        seq[0] += 1
        return f'压测新课{time.time_ns()}{seq[0]}'

    def full_upload():
        upload(scratch['id'], f'w{time.time_ns()}.pdf', f'write {time.time_ns()} {rnd.random()}'.encode() * 30)

    def review_one():
        r = upload(scratch['id'], f'rv{time.time_ns()}.pdf', f'review {time.time_ns()} {rnd.random()}'.encode() * 30, {'status': 'pending'})
        api('POST', f"/resources/{r['id']}/review", {'action': 'approve', 'note': ''})

    # ---- writes: every one the import account can do locally, on objects of its own.
    status = {}

    def w(label, method, path_fn, body_fn=lambda: None):
        def fn():
            code, _ = req(method, '/api' + path_fn(), body_fn())
            status.setdefault(label, {})
            status[label][code] = status[label].get(code, 0) + 1
        return label, fn

    g2 = groups[1]['id']
    sc2 = api('POST', '/nodes', {'parent': g2, 'kind': 'course', 'name': next_name()})['id']
    mine = [upload(scratch['id'], f'm{i}{time.time_ns()}.pdf', f'mine {i} {time.time_ns()}'.encode() * 30)['id'] for i in range(4)]
    want = next((x for x in api('GET', '/wants?status=mine') if x.get('status') == 'open'), None)
    if want is None:
        want = api('POST', '/wants', {'title': '压测求资料' + str(time.time_ns()), 'body': '', 'node': course['id']})
    col = api('POST', '/collections', {'title': '压测收藏', 'note': ''})
    ser = api('POST', '/series', {'node': scratch['id'], 'title': '压测合集', 'items': mine[:2]})
    flip = {}

    def toggle(k):
        flip[k] = 1 - flip.get(k, 0)
        return flip[k]

    def token_cycle():
        code, out = req('POST', '/api/me/tokens', {'name': '压测'})
        if code == 200:
            body = json.loads(out)
            tid = (body.get('token') or body)['id'] if isinstance(body, dict) else body[1]['id']
            req('DELETE', f'/api/me/tokens/{tid}')

    def node_cycle():
        n = api('POST', '/nodes', {'parent': g2, 'kind': 'course', 'name': next_name()})
        req('PATCH', f"/api/nodes/{n['id']}", {'name': n['name'] + '改'})
        req('DELETE', f"/api/nodes/{n['id']}")

    def merge_cycle():
        a = api('POST', '/nodes', {'parent': g2, 'kind': 'course', 'name': next_name()})
        b = api('POST', '/nodes', {'parent': g2, 'kind': 'course', 'name': next_name()})
        req('POST', f"/api/nodes/{a['id']}/merge", {'into': b['id']})

    def link_cycle():
        l = api('POST', '/links', {'title': '压测', 'url': f'https://example.org/{time.time_ns()}', 'note': '', 'sort': 9})
        req('PATCH', f"/api/links/{l['id']}", {'title': '压测2', 'url': l['url'], 'note': '', 'sort': 9})
        req('DELETE', f"/api/links/{l['id']}")

    def suggestion_cycle():
        code, out = req('POST', '/api/links/suggestions', {'title': '压测推荐', 'url': f'https://example.net/{time.time_ns()}', 'note': ''})
        status.setdefault('link suggest + review', {})[code] = status.get('link suggest + review', {}).get(code, 0) + 1
        if code == 200:
            req('POST', f"/api/links/suggestions/{json.loads(out)['id']}/review", {'approve': False, 'reason': '压测', 'note': '压测'})

    def collection_cycle():
        c = api('POST', '/collections', {'title': '压测临时', 'note': ''})
        req('PATCH', f"/api/collections/{c['id']}", {'title': '压测临时2', 'note': ''})
        req('DELETE', f"/api/collections/{c['id']}")

    def series_cycle():
        items = mine[:2] if toggle('ser') else mine[:3]
        req('PUT', f"/api/series/{ser['id']}", {'title': '压测合集', 'items': items})

    def want_cycle():
        code, out = req('POST', '/api/wants', {'title': '压测求' + str(time.time_ns()), 'body': '', 'node': course['id']})
        status.setdefault('want post + close', {})[code] = status.get('want post + close', {}).get(code, 0) + 1
        if code == 200:
            req('POST', f"/api/wants/{json.loads(out)['id']}/status", {'status': 'closed'})

    def want_reply_cycle():
        code, out = req('POST', f"/api/wants/{want['id']}/replies", {'body': '压测回复：看这里', 'resource': mine[0]})
        status.setdefault('want reply + delete', {})[code] = status.get('want reply + delete', {}).get(code, 0) + 1
        r = json.loads(out) if code == 200 and out else None
        rid = r.get('id') if isinstance(r, dict) else None
        if rid:
            req('DELETE', f'/api/want-replies/{rid}')

    writes = [
        ('POST /api/nodes (new course)', lambda: api('POST', '/nodes', {'parent': g2, 'kind': 'course', 'name': next_name()})),
        ('node create + rename + delete', node_cycle),
        ('node merge (2 new courses)', merge_cycle),
        ('upload: begin + part + publish', full_upload),
        ('upload pending + approve', review_one),
        w('POST /api/uploads (begin only)', 'POST', lambda: '/uploads', lambda: {'filename': 'x.pdf', 'mime': 'application/pdf', 'parts': [{'size': 1000, 'sha256': hashlib.sha256(str(time.time_ns()).encode()).hexdigest()}]}),
        w('POST /api/resources/preview-name', 'POST', lambda: '/resources/preview-name', lambda: {'node': course['id'], 'time': '2023', 'type_word': '期末试卷', 'ext': 'pdf'}),
        w('PATCH /api/resources/<id> (edit)', 'PATCH', lambda: f'/resources/{mine[0]}', lambda: {'node': scratch['id'], 'time': str(2010 + toggle('ed')), 'type_word': '课件', 'extra': 'm0'}),
        w('POST /api/resources/move', 'POST', lambda: '/resources/move', lambda: {'ids': [mine[1]], 'node': sc2 if toggle('mv') else scratch['id']}),
        w('POST review remove/restore', 'POST', lambda: f'/resources/{mine[2]}/review', lambda: {'action': 'remove' if toggle('rv') else 'restore', 'note': '压测'}),
        w('POST /api/resources/<id>/report', 'POST', lambda: f'/resources/{mine[3]}/report', lambda: {'reason': '压测投诉', 'contact': ''}),
        w('PUT rating', 'PUT', lambda: f'/resources/{pick_res()}/rating', lambda: {'stars': rnd.randint(1, 5)}),
        w('POST comment (limited per user)', 'POST', lambda: f'/resources/{pick_res()}/comments', lambda: {'body': '压测留言：这份资料很有用'}),
        w('POST /api/feedback', 'POST', lambda: '/feedback', lambda: {'body': '压测反馈内容', 'contact': '', 'page': '/'}),
        w('PATCH /api/me (nickname)', 'PATCH', lambda: '/me', lambda: {'nickname': f'压测{toggle("nk")}'}),
        ('token create + revoke', token_cycle),
        w('PUT follow on/off', 'PUT', lambda: f"/nodes/{course['id']}/follow", lambda: {'on': bool(toggle('fol'))}),
        w('POST /api/notices/read', 'POST', lambda: '/notices/read', lambda: {}),
        w('PUT /api/announcement', 'PUT', lambda: '/announcement', lambda: {'text': f'压测公告 {time.time_ns() % 1000}'}),
        ('link add + edit + delete', link_cycle),
        ('link suggest + review', suggestion_cycle),
        ('want post + close', want_cycle),
        w('PUT want vote', 'PUT', lambda: f"/wants/{want['id']}/vote", lambda: {'on': bool(toggle('vote'))}),
        ('want reply + delete', want_reply_cycle),
        ('collection create + rename + delete', collection_cycle),
        w('PUT collection item on/off', 'PUT', lambda: f"/collections/{col['id']}/items/{pick_res()}", lambda: {'on': bool(toggle('ci'))}),
        w('POST collection share on/off', 'POST', lambda: f"/collections/{col['id']}/share", lambda: {'on': bool(toggle('sh'))}),
        ('PUT series (change)', series_cycle),
        w('POST /api/inbox', 'POST', lambda: '/inbox', lambda: {}),
        w('POST review batch release', 'POST', lambda: '/review/batch/release', lambda: {}),
    ]

    report = {'data': {'courses': len(courses), 'files_in_course': len(res), 'biggest_course': biggest['count']}, 'latency': {}, 'throughput': {}}
    print(f"data: {len(courses)} courses with files; reads use one with {len(res)} files, the biggest has {biggest['count']}\n")
    print(f"{'endpoint':<40}{'p50':>9}{'p90':>9}{'p99':>9}   (ms, {A.n} requests each; writes {max(20, A.n // 10)})")
    for label, fn in reads + writes:
        n = A.n if (label, fn) in reads else max(20, A.n // 10)
        for _ in range(3):
            fn()
        v = [timed(fn) for _ in range(n)]
        report['latency'][label] = {'p50': pct(v, 50), 'p90': pct(v, 90), 'p99': pct(v, 99), 'n': n}
        flag = '  <-- slow' if pct(v, 90) > 50 else ''
        bad = {k: c for k, c in status.get(label, {}).items() if k >= 400}
        if bad:
            flag += f'  HTTP {bad}'
            report['latency'][label]['errors'] = bad
        print(f'{label:<40}{pct(v, 50):>9.1f}{pct(v, 90):>9.1f}{pct(v, 99):>9.1f}{flag}')

    mix = [fn for label, fn in reads if 'staff' not in label and 'biggest' not in label]

    def load(seconds, background=None):
        stop = time.time() + seconds
        lat, errs = [], [0]
        lock = threading.Lock()

        def client():
            r = random.Random()
            while time.time() < stop:
                fn = r.choice(mix)
                t = time.perf_counter()
                try:
                    fn()
                except Exception:
                    errs[0] += 1
                d = (time.perf_counter() - t) * 1000
                with lock:
                    lat.append(d)

        bg = None
        if background:
            bg = threading.Thread(target=background, args=(stop,), daemon=True)
            bg.start()
        with cf.ThreadPoolExecutor(A.threads) as ex:
            for _ in range(A.threads):
                ex.submit(client)
        if bg:
            bg.join()
        return {'req_per_s': len(lat) / seconds, 'p50': pct(lat, 50), 'p99': pct(lat, 99), 'errors': errs[0]}

    def uploader(stop):
        with cf.ThreadPoolExecutor(4) as ex:
            while time.time() < stop:
                list(ex.map(lambda _: full_upload(), range(4)))

    print(f'\nmixed reads, {A.threads} clients, {A.seconds:.0f} s:')
    for label, bgfn in [('reads only', None), ('reads during a batch upload (4 uploaders)', uploader)]:
        r = load(A.seconds, bgfn)
        report['throughput'][label] = r
        print(f"  {label:<44} {r['req_per_s']:>7.0f} req/s   p50 {r['p50']:.1f} ms   p99 {r['p99']:.1f} ms   errors {r['errors']}")
    if A.json:
        with open(A.json, 'w', encoding='utf-8') as f:
            json.dump(report, f, ensure_ascii=False, indent=1)
        print(f'\nwritten to {A.json}')


seed() if A.mode == 'seed' else run()
