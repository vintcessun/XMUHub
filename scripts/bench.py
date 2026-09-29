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
        for k in range(20):
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
    print(f'{len(ids)} files in {time.time() - t0:.0f} s; 300 ratings')


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

    writes = [
        ('POST /api/nodes (new course)', lambda: api('POST', '/nodes', {'parent': groups[1]['id'], 'kind': 'course', 'name': next_name()})),
        ('upload: begin + part + publish', full_upload),
        ('upload pending + approve', review_one),
        ('PUT rating', lambda: api('PUT', f'/resources/{pick_res()}/rating', {'stars': rnd.randint(1, 5)})),
    ]

    report = {'data': {'courses': len(courses), 'files_in_course': len(res), 'biggest_course': biggest['count']}, 'latency': {}, 'throughput': {}}
    print(f"data: {len(courses)} courses with files; biggest course has {len(res)} files\n")
    print(f"{'endpoint':<36}{'p50':>9}{'p90':>9}{'p99':>9}   (ms, {A.n} requests each; writes {max(20, A.n // 10)})")
    for label, fn in reads + writes:
        n = A.n if (label, fn) in reads else max(20, A.n // 10)
        for _ in range(3):
            fn()
        v = [timed(fn) for _ in range(n)]
        report['latency'][label] = {'p50': pct(v, 50), 'p90': pct(v, 90), 'p99': pct(v, 99), 'n': n}
        flag = '  <-- slow' if pct(v, 90) > 50 else ''
        print(f'{label:<36}{pct(v, 50):>9.1f}{pct(v, 90):>9.1f}{pct(v, 99):>9.1f}{flag}')

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
