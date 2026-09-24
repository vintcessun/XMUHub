//! First-page thumbnails for resource lists, made inside GitHub.
//!
//! Like `transfer`, this dispatches a GitHub Actions workflow in the storage account's
//! `XMUHub-transfer` repo: the runner downloads each file straight from GitHub, renders its
//! first page (PDF via poppler, Office via LibreOffice, archives via their first document,
//! text via Pillow) into a small WebP and uploads it as a release asset. Neither our
//! server nor anyone's machine touches the files. Results are attached per blob.

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::json;

use xmuhub_core::Hub;
use xmuhub_core::model::{Location, now};
use xmuhub_core::storage::github::GitHubBackend;

use crate::transfer::REPO;

const WORKFLOW: &str = "thumbs.yml";
const STATE_KEY: &str = "thumbs.job";
const BATCH_FILES: usize = 300;
const BATCH_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const RUN_TIMEOUT: i64 = 6 * 3600;

const WORKFLOW_YML: &str = r#"# Managed by XMUHub (crates/xmuhub-server/src/thumbs.rs) — do not edit by hand.
name: thumbs
on:
  workflow_dispatch:
    inputs:
      job:
        description: job id
        required: true
permissions:
  contents: write
concurrency:
  group: thumbs
jobs:
  make:
    runs-on: ubuntu-24.04
    timeout-minutes: 330
    steps:
      - uses: actions/checkout@v4
      - name: install renderers
        run: |
          sudo apt-get update -qq
          sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends \
            poppler-utils libreoffice-writer libreoffice-impress libreoffice-calc \
            fonts-noto-cjk fonts-wqy-microhei 7zip python3-pil > /dev/null
          sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq 7zip-rar > /dev/null || echo "no rar support"
      - name: make thumbnails
        env:
          GH_TOKEN: ${{ github.token }}
        run: python3 thumbs.py "jobs/thumbs-${{ inputs.job }}.json"
      - name: commit results
        run: |
          git config user.name xmuhub-thumbs
          git config user.email thumbs@users.noreply.github.com
          git add results
          git commit -m "thumbs ${{ inputs.job }}"
          for i in 1 2 3; do git pull --rebase && git push && break; sleep 5; done
"#;

const THUMBS_PY: &str = r#"# Managed by XMUHub — renders first-page thumbnails into a release of this repository.
import json, os, shutil, subprocess, sys, tempfile, time, urllib.parse, urllib.request, zipfile
from PIL import Image, ImageDraw, ImageFont

job = json.load(open(sys.argv[1], encoding='utf8'))
repo = os.environ['GITHUB_REPOSITORY']
H = {'Authorization': 'Bearer ' + os.environ['GH_TOKEN'], 'Accept': 'application/vnd.github+json', 'User-Agent': 'xmuhub-thumbs'}

def call(method, url, body=None, data=None, headers=None):
    h = dict(H, **(headers or {}))
    if body is not None:
        data = json.dumps(body).encode(); h['Content-Type'] = 'application/json'
    with urllib.request.urlopen(urllib.request.Request(url, data=data, method=method, headers=h), timeout=300) as r:
        raw = r.read()
        return json.loads(raw) if raw else None

try:
    rel = call('GET', f'https://api.github.com/repos/{repo}/releases/tags/{job["tag"]}')
except urllib.error.HTTPError:
    rel = call('POST', f'https://api.github.com/repos/{repo}/releases', {'tag_name': job['tag'], 'name': job['tag'], 'make_latest': 'false'})
have = {a['name'] for a in rel.get('assets', [])}

PDF = {'pdf'}
IMGS = {'png', 'jpg', 'jpeg', 'gif', 'bmp', 'webp', 'tif', 'tiff'}
DOCS = {'doc', 'docx', 'docm', 'dot', 'dotx', 'rtf', 'odt', 'wps', 'ppt', 'pptx', 'pptm', 'pps', 'ppsx', 'odp', 'dps',
        'xls', 'xlsx', 'xlsm', 'xlsb', 'ods', 'et', 'csv'}
TEXT = {'txt', 'md', 'markdown', 'py', 'c', 'cpp', 'cc', 'h', 'hpp', 'java', 'js', 'ts', 'html', 'htm', 'css', 'tex',
        'json', 'xml', 'm', 'r', 'sql', 'log', 'go', 'rs', 'sh', 'yaml', 'yml', 'ini', 'v', 'asm'}
ARCH = {'zip', 'rar', '7z', 'tar', 'gz', 'tgz', 'bz2', 'xz'}
PREFER = ['pdf', 'docx', 'doc', 'pptx', 'ppt', 'xlsx', 'xls', 'jpg', 'jpeg', 'png', 'md', 'txt']
SEVENZ = shutil.which('7z') or shutil.which('7zz')
FONT = next((f for f in ['/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc',
                         '/usr/share/fonts/truetype/wqy/wqy-microhei.ttc'] if os.path.exists(f)), None)

class Unsupported(Exception):
    pass

def run(cmd, timeout):
    try:
        subprocess.run(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=timeout, check=False)
    except subprocess.TimeoutExpired:
        pass

def ext_of(p):
    return os.path.splitext(p)[1].lstrip('.').lower()

def sniff(path):
    head = open(path, 'rb').read(8)
    if head.startswith(b'%PDF'): return 'pdf'
    if head.startswith(b'PK'): return 'zip'
    if head.startswith(b'Rar!'): return 'rar'
    if head.startswith(b'7z\xbc\xaf'): return '7z'
    if head.startswith(b'\x89PNG'): return 'png'
    if head.startswith(b'\xff\xd8'): return 'jpg'
    if head.startswith(b'\xd0\xcf\x11\xe0'): return 'doc'
    return ''

def pdf_page(path, d):
    out = os.path.join(d, 'page')
    run(['pdftoppm', '-f', '1', '-l', '1', '-r', '60', '-png', path, out], 120)
    pages = sorted(f for f in os.listdir(d) if f.startswith('page') and f.endswith('.png'))
    return os.path.join(d, pages[0]) if pages else None

def office(path, d):
    run(['soffice', '--headless', '--norestore', '--convert-to', 'pdf', '--outdir', d, path], 240)
    pdf = os.path.join(d, os.path.splitext(os.path.basename(path))[0] + '.pdf')
    return pdf_page(pdf, d) if os.path.exists(pdf) else None

def text_image(path, d):
    raw = open(path, 'rb').read(16000)
    for enc in ('utf-8', 'gb18030', 'latin-1'):
        try:
            s = raw.decode(enc); break
        except UnicodeDecodeError:
            continue
    img = Image.new('RGB', (600, 800), 'white')
    draw = ImageDraw.Draw(img)
    font = ImageFont.truetype(FONT, 18) if FONT else ImageFont.load_default()
    y = 16
    for line in s.replace('\t', '    ').splitlines()[:38]:
        draw.text((16, y), line[:48], fill=(30, 30, 30), font=font)
        y += 20
    out = os.path.join(d, 'text.png'); img.save(out)
    return out

def epub_cover(path, d):
    with zipfile.ZipFile(path) as z:
        imgs = [i for i in z.infolist() if ext_of(i.filename) in ('jpg', 'jpeg', 'png')]
        if not imgs: raise Unsupported('epub without images')
        cover = next((i for i in imgs if 'cover' in i.filename.lower()), max(imgs, key=lambda i: i.file_size))
        out = os.path.join(d, 'cover.' + ext_of(cover.filename))
        open(out, 'wb').write(z.read(cover))
        return out

def archive(path, d, depth):
    if not SEVENZ: raise Unsupported('no 7z')
    x = os.path.join(d, 'x'); os.makedirs(x, exist_ok=True)
    run([SEVENZ, 'x', '-y', '-pxmuhub', '-o' + x, path], 300)
    files = [os.path.join(r, f) for r, _, fs in os.walk(x) for f in fs]
    for want in PREFER:
        cands = sorted(f for f in files if ext_of(f) == want)
        for c in cands[:3]:
            sub = tempfile.mkdtemp(dir=d)
            try:
                img = render(c, want, sub, depth + 1)
                if img: return img
            except Exception:
                pass
    raise Unsupported('nothing previewable inside')

def render(path, ext, d, depth=0):
    if ext not in PDF | IMGS | DOCS | TEXT | ARCH | {'epub'}:
        ext = sniff(path) or ext
    if ext in PDF: return pdf_page(path, d)
    if ext in IMGS: return path
    if ext in DOCS: return office(path, d)
    if ext == 'epub': return epub_cover(path, d)
    if ext in ARCH and depth < 2: return archive(path, d, depth)
    if ext in TEXT: return text_image(path, d)
    raise Unsupported('format ' + ext)

def thumbnail(src, out):
    im = Image.open(src); im.load()
    if im.mode in ('RGBA', 'LA', 'P'):
        im = im.convert('RGBA'); bg = Image.new('RGB', im.size, 'white'); bg.paste(im, mask=im.split()[-1]); im = bg
    im = im.convert('RGB')
    w = 360; h = max(1, round(im.height * w / im.width))
    im = im.resize((w, h), Image.LANCZOS)
    if h > 480: im = im.crop((0, 0, w, 480))
    im.save(out, 'WEBP', quality=72, method=6)

results = []
for it in job['items']:
    name = it['key'] + '.webp'
    if name in have:
        results.append({'key': it['key'], 'name': name}); continue
    d = tempfile.mkdtemp()
    try:
        src = os.path.join(d, 'in.' + (it['ext'] or 'bin'))
        with open(src, 'wb') as f:
            for u in it['urls']:
                for attempt in range(3):
                    try:
                        with urllib.request.urlopen(urllib.request.Request(u, headers={'User-Agent': 'xmuhub-thumbs'}), timeout=300) as r:
                            shutil.copyfileobj(r, f)
                        break
                    except Exception:
                        if attempt == 2: raise
                        time.sleep(5)
        img = render(src, it['ext'], d)
        if not img: raise RuntimeError('render produced nothing')
        out = os.path.join(d, name)
        thumbnail(img, out)
        up = f'https://uploads.github.com/repos/{repo}/releases/{rel["id"]}/assets?name={urllib.parse.quote(name)}'
        a = call('POST', up, data=open(out, 'rb').read(), headers={'Content-Type': 'image/webp'})
        results.append({'key': it['key'], 'name': a['name']})
    except Unsupported as e:
        results.append({'key': it['key'], 'error': str(e)[:200], 'permanent': True})
    except Exception as e:
        results.append({'key': it['key'], 'error': str(e)[:200]})
    finally:
        shutil.rmtree(d, ignore_errors=True)

os.makedirs('results', exist_ok=True)
json.dump({'tag': job['tag'], 'results': results}, open(f'results/thumbs-{job["id"]}.json', 'w', encoding='utf8'), ensure_ascii=False)
print(sum(1 for r in results if 'error' not in r), 'ok,', sum(1 for r in results if 'error' in r), 'failed')
"#;

#[derive(Serialize, Deserialize, Clone)]
struct Current {
    id: String,
    tag: String,
    dispatched_at: i64,
    files: usize,
}

#[derive(Deserialize)]
struct ResultsFile {
    tag: String,
    #[serde(default)]
    results: Vec<ResultItem>,
}

#[derive(Deserialize)]
struct ResultItem {
    key: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    error: String,
    #[serde(default)]
    permanent: bool,
}

pub fn current_job(hub: &Hub) -> Option<serde_json::Value> {
    let bytes = hub.meta_get(STATE_KEY).ok()??;
    let c: Current = serde_json::from_slice(&bytes).ok()?;
    Some(json!({ "id": c.id, "files": c.files, "dispatched_at": c.dispatched_at }))
}

async fn ensure_setup(gh: &GitHubBackend) -> anyhow::Result<()> {
    gh.ensure_repo(REPO, false).await?;
    gh.write_file(REPO, ".github/workflows/thumbs.yml", WORKFLOW_YML.as_bytes(), "update thumbs workflow").await?;
    gh.write_file(REPO, "thumbs.py", THUMBS_PY.as_bytes(), "update thumbs script").await?;
    Ok(())
}

async fn apply(hub: &Arc<Hub>, gh: &GitHubBackend, cur: &Current, res: ResultsFile) -> anyhow::Result<()> {
    let Some((release_id, assets)) = gh.release_assets(REPO, &res.tag).await? else {
        anyhow::bail!("release {} missing", res.tag);
    };
    let (mut ok, mut failed) = (0, 0);
    for it in res.results {
        let h = hub.clone();
        let key = it.key.clone();
        let outcome = if it.error.is_empty() {
            match assets.iter().find(|a| a.1 == it.name) {
                Some((asset_id, name, _, _)) => {
                    let loc = Location::GitHub { owner: gh.owner_login().to_string(), repo: REPO.into(), release_id, tag: res.tag.clone(), asset_id: *asset_id, name: name.clone() };
                    ok += 1;
                    (Some(loc), false)
                }
                None => {
                    failed += 1;
                    (None, false)
                }
            }
        } else {
            tracing::debug!(key = %it.key, "thumbnail failed: {}", it.error);
            failed += 1;
            (None, it.permanent)
        };
        let _ = tokio::task::spawn_blocking(move || h.set_thumb(&key, outcome.0, outcome.1)).await;
    }
    tracing::info!(job = %cur.id, ok, failed, "thumbnail run applied");
    Ok(())
}

async fn tick(hub: &Arc<Hub>, gh: &GitHubBackend) -> anyhow::Result<()> {
    if let Some(bytes) = hub.meta_get(STATE_KEY)?.filter(|b| !b.is_empty()) {
        let cur: Current = serde_json::from_slice(&bytes)?;
        if let Some((raw, _)) = gh.read_file(REPO, &format!("results/thumbs-{}.json", cur.id)).await? {
            let res: ResultsFile = serde_json::from_slice(&raw)?;
            apply(hub, gh, &cur, res).await?;
            hub.meta_put(STATE_KEY, b"")?;
        } else if now() - cur.dispatched_at > RUN_TIMEOUT {
            tracing::warn!(job = %cur.id, "thumbnail run timed out; its files will be retried");
            hub.meta_put(STATE_KEY, b"")?;
        }
        return Ok(());
    }
    let items = hub.thumb_backlog(BATCH_FILES, BATCH_BYTES);
    if items.is_empty() {
        return Ok(());
    }
    ensure_setup(gh).await?;
    let id = format!("{}", now());
    let tag = format!("thumbs-{id}");
    let job = json!({ "id": id, "tag": tag, "items": items });
    gh.write_file(REPO, &format!("jobs/thumbs-{id}.json"), serde_json::to_vec_pretty(&job)?.as_slice(), &format!("thumbs {id}")).await?;
    tokio::time::sleep(Duration::from_secs(5)).await;
    gh.dispatch(REPO, WORKFLOW, json!({ "job": id })).await?;
    let cur = Current { id: id.clone(), tag, dispatched_at: now(), files: items.len() };
    hub.meta_put(STATE_KEY, &serde_json::to_vec(&cur)?)?;
    tracing::info!(job = %id, files = items.len(), "thumbnail run dispatched");
    Ok(())
}

pub fn spawn(hub: Arc<Hub>, gh: Arc<GitHubBackend>) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(120)).await;
        let mut every = tokio::time::interval(Duration::from_secs(10 * 60));
        loop {
            every.tick().await;
            if let Err(e) = tick(&hub, &gh).await {
                tracing::warn!("thumbs: {e}");
            }
        }
    });
}

/// Runs one step right away (admin "check now").
pub async fn kick(hub: &Arc<Hub>, gh: &GitHubBackend) -> anyhow::Result<()> {
    tick(hub, gh).await
}
