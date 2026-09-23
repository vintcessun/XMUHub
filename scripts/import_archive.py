#!/usr/bin/env python3
"""Import the categorised archive (目录即分类，文件名即元数据) into XMUHub.

  python scripts/import_archive.py ARCHIVE.zip --base https://xmu.vintces.icu [--dry-run] [--limit N]

* Directories become category nodes (A/B/C/D → groups → courses → levels); the fixed
  A-course folders 01-04 become type tags instead of nodes.
* File names `课程_时间_类型(详情)_vN（不确定）.ext` are parsed into structured name parts.
* Status: names whose ORIGINAL name (重命名对照表.csv) says 勿外传/仅内部/密码 → restricted;
  "（不确定）" → pending review (uncertain); everything else → published.
* Bytes go straight from this machine to GitHub with the storage token — the XMUHub server
  only reserves slots and verifies assets, so the import costs the server no bandwidth.
* Progress is saved to a state file next to the archive, so re-running resumes.

Needs .secrets/upload.env (XMUHUB_SCRIPT_TOKEN) and .secrets/github.env (GH_STORE_TOKEN).
"""

import argparse, base64, csv, hashlib, io, json, os, re, sys, time, urllib.error, urllib.parse, urllib.request, zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MAX_PART = 95 * 1024 * 1024
TYPE_TAGS = {
    '期中试卷': 'T1', '期末试卷': 'T1', '小测': 'T1', '往年试卷': 'T1', '答案': 'T2',
    '重点': 'T3', '提纲': 'T3', '笔记': 'T3', '单词表': 'T3', '题库': 'T4', '思考题': 'T4',
    '课件': 'T5', '讲义': 'T5', '资料': 'T5', '实验报告': 'T6', '合集': 'T7', '教材': 'T8', '模板': 'T9', '工具': 'T9',
}
BUCKET_DIRS = {'01-真题与答案': 1, '02-提纲笔记': 2, '03-题库刷题': 3, '04-课件与拓展': 4}
RESTRICT_WORDS = ('勿外传', '仅内部', '内部使用', '内部学习', '密码')
# Search aliases (群内俗称) from the classification scheme, keyed by node code.
ALIASES = {
    'A1-1': ['思修', '思想道德修养'], 'A1-2': ['史纲', '近代史'], 'A1-3': ['马原'], 'A1-4': ['毛概'],
    'A1-5': ['习概', '习思想'], 'A1-6': ['形策', '行策'], 'A1-7': ['军理'], 'A2-5': ['线代'], 'A2-6': ['概统', '概率论'],
    'A2-7': ['高数', '文科数学'], 'A2-8': ['中科实'], 'A3': ['大物'], 'A4-6': ['中科实'], 'A5-1': ['C语言', 'C++'],
    'A6-4': ['四级', '六级', 'CET'], 'A7-1': ['普生'], 'A3-4': ['大物实验'],
}
LEVEL_LABELS = {'线代I': '线性代数I', '线代II': '线性代数II', '概统I': '概率统计I', '概统II': '概率统计II'}


def env_file(name):
    out = {}
    for line in (ROOT / '.secrets' / name).read_text(encoding='utf8').splitlines():
        if '=' in line and not line.startswith('#'):
            k, v = line.split('=', 1)
            out[k.strip()] = v.strip()
    return out


def zname(info):
    if info.flag_bits & 0x800:
        return info.filename
    try:
        return info.filename.encode('cp437').decode('gbk')
    except Exception:
        return info.filename


class Api:
    def __init__(self, base, token):
        self.base, self.token = base.rstrip('/'), token

    def call(self, method, path, body=None):
        data = None if body is None else json.dumps(body).encode()
        req = urllib.request.Request(self.base + '/api' + path, data=data, method=method, headers={
            'Authorization': 'Bearer ' + self.token, 'X-XMUHub': '1', 'Content-Type': 'application/json',
            'User-Agent': 'XMUHub-importer/1.0'})
        for attempt in range(6):
            try:
                with urllib.request.urlopen(req, timeout=120) as r:
                    return json.loads(r.read() or b'null')
            except urllib.error.HTTPError as e:
                msg = e.read().decode('utf8', 'replace')
                # 502/503 while the server restarts (deploys) — wait and retry.
                if e.code in (502, 503, 504) and attempt < 5:
                    time.sleep(10)
                    continue
                raise RuntimeError(f'{method} {path} -> HTTP {e.code}: {msg[:300]}') from None
            except (urllib.error.URLError, ConnectionError, TimeoutError):
                if attempt == 5:
                    raise
                time.sleep(10)


# ---------------------------------------------------------------- parsing

def node_spec(parts, parent_spec):
    """(kind, code, name, label, bucketed, sort) for directory `parts[-1]`."""
    d = parts[-1]
    depth = len(parts)
    section = parts[0][0]
    if depth == 1:
        code, name = d.split('-', 1)
        return 'section', code, name, name, False, 'ABCD'.index(code) + 1
    m = re.match(r'^([A-D]\d+(?:-\d+)?)-(.+)$', d)
    code, name = (m.group(1), m.group(2)) if m else ('', d)
    if depth == 2:
        return 'group', code, name, '', False, 0
    if section == 'A' and depth == 3:
        label = name
        if name.startswith('未分层-'):
            label = name.split('-', 1)[1]
            name = f'未分层（{label}）'
        label = re.sub(r'（中科实）$', '', label)
        return 'course', code, name, label, True, 0
    if section == 'A' and depth == 4:
        return 'level', '', name, LEVEL_LABELS.get(name, ''), True, 0
    if section == 'B' and depth == 3:
        return 'course', '', name, name, False, 0
    return 'group', code, name, '', False, 0


def parse_name(fname):
    stem, ext = (fname.rsplit('.', 1) + [''])[:2] if '.' in fname else (fname, '')
    ext = ext.lower()
    uncertain = '（不确定）' in stem
    stem = stem.replace('（不确定）', '')
    segs = stem.split('_')
    version = None
    while len(segs) > 1 and re.fullmatch(r'v\d+', segs[-1]):
        version = max(version or 0, int(segs.pop()[1:]))
    course = segs[0]
    rest = segs[1:]
    type_seg = rest.pop() if rest else '资料'
    m = re.fullmatch(r'(.+?)\((.+)\)', type_seg)
    type_word, details = (m.group(1), [x.strip() for x in m.group(2).split('、')]) if m else (type_seg, [])
    paper = next((x for x in details if re.fullmatch(r'[A-C]卷', x)), '')
    with_answer = '含答案' in details
    extra = '、'.join(x for x in details if x not in (paper, '含答案'))
    time_ = ''
    if rest:
        cand = '-'.join(rest)
        if re.fullmatch(r'\d{4}(-\d{4})?[秋春暑]?|\d{6}', cand):
            time_ = cand
        else:
            extra = '、'.join(filter(None, [cand, extra]))
    return dict(course=course, time=time_, type_word=type_word, paper=paper, with_answer=with_answer,
                extra=extra[:20], version=version, uncertain=uncertain, ext=ext)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('archive')
    ap.add_argument('--base', default='https://xmu.vintces.icu')
    ap.add_argument('--dry-run', action='store_true')
    ap.add_argument('--limit', type=int, default=0)
    ap.add_argument('--proxy', default='', help='proxy for GitHub uploads, e.g. http://127.0.0.1:7890')
    args = ap.parse_args()

    up = env_file('upload.env')
    gh = env_file('github.env')
    api = Api(args.base, up['XMUHUB_SCRIPT_TOKEN'])
    z = zipfile.ZipFile(args.archive)
    infos = [(zname(i), i) for i in z.infolist()]
    prefix = infos[0][0].split('/')[0] + '/'

    # 重命名对照表: current name → original names (several originals may share one new name).
    originals = {}
    csv_info = next((i for n, i in infos if n.endswith('重命名对照表.csv')), None)
    if csv_info:
        raw = z.read(csv_info)
        text = raw.decode('utf-8-sig', 'replace') if raw[:3] == b'\xef\xbb\xbf' else raw.decode('gbk', 'replace')
        for row in csv.DictReader(io.StringIO(text)):
            originals.setdefault(row.get('新文件名', ''), []).append(row.get('原文件名', ''))

    files, dirs = [], set()
    for n, i in infos:
        rel = n[len(prefix):] if n.startswith(prefix) else n
        if not rel or i.is_dir():
            continue
        parts = rel.split('/')
        if len(parts) < 2 or parts[-1] == '.gitkeep':
            if len(parts) >= 2:
                dirs.add(tuple(p for p in parts[:-1] if p not in BUCKET_DIRS))
            continue
        bucket = next((BUCKET_DIRS[p] for p in parts[:-1] if p in BUCKET_DIRS), 0)
        node_parts = tuple(p for p in parts[:-1] if p not in BUCKET_DIRS)
        dirs.add(node_parts)
        files.append((rel, i, node_parts, bucket))
    all_dirs = set()
    for d in dirs:
        for k in range(1, len(d) + 1):
            all_dirs.add(d[:k])

    state_path = Path(args.archive).with_suffix('.import-state.json')
    state = json.loads(state_path.read_text(encoding='utf8')) if state_path.exists() else {'nodes': {}, 'files': {}}
    save = lambda: state_path.write_text(json.dumps(state, ensure_ascii=False, indent=1), encoding='utf8')

    # ------------------------------------------------------------ nodes
    print(f'{len(all_dirs)} category nodes, {len(files)} files')
    for d in sorted(all_dirs, key=lambda d: (len(d), d)):
        key = '/'.join(d)
        if key in state['nodes']:
            continue
        kind, code, name, label, bucketed, sort = node_spec(d, None)
        parent = state['nodes'].get('/'.join(d[:-1])) if len(d) > 1 else None
        body = dict(parent=parent, kind=kind, code=code, name=name, label=label, bucketed=bucketed, sort=sort,
                    aliases=ALIASES.get(code, []))
        if args.dry_run:
            print('  node', key, '→', body)
            state['nodes'][key] = -1
            continue
        n = api.call('POST', '/nodes', body)
        state['nodes'][key] = n['id']
        save()
    print('nodes ready')

    # ------------------------------------------------------------ files
    opener = urllib.request.build_opener(*(
        [urllib.request.ProxyHandler({'https': args.proxy, 'http': args.proxy})] if args.proxy else [urllib.request.ProxyHandler({})]))
    # Versions of one restricted file (e.g. several copies of an encrypted 题库) are all restricted,
    # even when the rename table lost track of some originals.
    def base_of(rel):
        f = rel.split('/')[-1].replace('（不确定）', '')
        return re.sub(r'(_v\d+)+(?=\.[^.]+$|$)', '', f)
    restricted_bases = set()
    for rel, _, _, _ in files:
        f = rel.split('/')[-1]
        origs = originals.get(f.replace('（不确定）', ''), []) + originals.get(f, [])
        if any(w in o for o in origs + [f] for w in RESTRICT_WORDS):
            restricted_bases.add(base_of(rel))

    done = 0
    stats = {'published': 0, 'pending': 0, 'restricted': 0, 'skipped': 0}
    for rel, info, node_parts, bucket in sorted(files):
        if args.limit and done >= args.limit:
            break
        if rel in state['files']:
            continue
        fname = rel.split('/')[-1]
        p = parse_name(fname)
        clean_name = fname.replace('（不确定）', '')
        origs = originals.get(clean_name, []) + originals.get(fname, [])
        restricted = base_of(rel) in restricted_bases
        status = 'restricted' if restricted else ('pending' if p['uncertain'] else 'published')
        tag = TYPE_TAGS.get(p['type_word'], 'T5')
        if bucket == 1:
            tag = 'T2' if p['type_word'] == '答案' else 'T1'
        elif bucket == 2:
            tag = 'T3'
        elif bucket == 3:
            tag = 'T4'
        section = node_parts[0][0]
        if section == 'C' and p['type_word'] in ('教材', '资料'):
            tag = 'T8'
        if section == 'D' and p['type_word'] in ('模板', '工具'):
            tag = 'T9'
        node_id = state['nodes']['/'.join(node_parts)]
        meta = dict(node=node_id, course=p['course'], time=p['time'], type_word=p['type_word'], tag=tag,
                    paper=p['paper'], with_answer=p['with_answer'], extra=p['extra'], note='',
                    admin=dict(status=status, original_name=(' | '.join(dict.fromkeys(origs)) or fname)[:200], uncertain=p['uncertain'],
                               source='虾兵资料库归档（2026-09）', free_type=True))
        if args.dry_run:
            print(f'  {status:10} {tag} {rel}\n{"":13}→ {meta}')
            done += 1
            stats[status] += 1
            continue

        data = z.read(info)
        parts = [data[i:i + MAX_PART] for i in range(0, len(data), MAX_PART)]
        specs = [{'size': len(b), 'sha256': hashlib.sha256(b).hexdigest()} for b in parts]
        t0 = time.time()
        plan = api.call('POST', '/uploads', {'filename': clean_name, 'mime': '', 'parts': specs})
        if not plan['dedup']:
            for pp in plan['parts']:
                if pp['done']:
                    continue
                body = parts[pp['index']]
                for attempt in range(4):
                    target = pp['target'] if attempt == 0 else api.call('POST', f"/uploads/{plan['upload_id']}/parts/{pp['index']}/renew")['target']
                    url = target['url']
                    try:
                        if '/api/relay/upload?t=' in url:
                            # Decode the signed ticket and upload straight to GitHub.
                            payload = url.split('t=', 1)[1].split('.', 1)[0]
                            dest = json.loads(base64.urlsafe_b64decode(payload + '=' * (-len(payload) % 4)))['u']
                            dest = urllib.parse.quote(dest, safe=':/?=&')
                            req = urllib.request.Request(dest, data=body, method='POST', headers={
                                'Authorization': 'Bearer ' + gh['GH_STORE_TOKEN'], 'Accept': 'application/vnd.github+json',
                                'Content-Type': 'application/octet-stream', 'User-Agent': 'XMUHub-importer/1.0'})
                            with opener.open(req, timeout=1800) as r:
                                receipt = json.loads(r.read())
                        else:  # local dev backend
                            req = urllib.request.Request(api.base + url, data=body, method=target['method'], headers={'X-XMUHub': '1'})
                            with urllib.request.urlopen(req, timeout=600) as r:
                                receipt = json.loads(r.read() or b'{}')
                        api.call('POST', f"/uploads/{plan['upload_id']}/parts/{pp['index']}", {'asset_id': receipt.get('id')})
                        break
                    except Exception as e:
                        print(f'    retry {attempt + 1}: {e}')
                        if attempt == 3:
                            raise
                        time.sleep(5 * (attempt + 1))
        try:
            res = api.call('POST', '/resources', dict(upload_id=plan['upload_id'], **meta))
            state['files'][rel] = res['id']
            stats[status] += 1
            print(f"[{len(state['files'])}/{len(files)}] {status:10} {res['filename']}  ({len(data) / 1048576:.1f} MB, {time.time() - t0:.0f}s)")
        except RuntimeError as e:
            if 'HTTP 409' in str(e):
                state['files'][rel] = 0
                stats['skipped'] += 1
                print(f'  skip duplicate: {rel}')
            else:
                raise
        save()
        done += 1
    print('done', stats)


if __name__ == '__main__':
    main()
