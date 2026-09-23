//! Background transfer of repository-referenced files into our own storage.
//!
//! Imports from other repositories start as references (`Location::GitHubRepo`). This job
//! copies them into release assets of the storage account's `XMUHub-transfer` repo using a
//! GitHub Actions workflow there, so the bytes move inside GitHub: neither the XMUHub server
//! nor anyone's machine downloads them. When a run finishes, its results file is verified
//! against the release assets (size + GitHub's sha256 digest) and the copies are attached as
//! the preferred replicas.

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::json;

use xmuhub_core::Hub;
use xmuhub_core::model::{Location, now};
use xmuhub_core::storage::github::GitHubBackend;
use xmuhub_core::text::ascii_filename;

pub const REPO: &str = "XMUHub-transfer";
const WORKFLOW: &str = "transfer.yml";
const STATE_KEY: &str = "transfer.job";
/// Per run: stays well inside one release's asset cap and the 6 h job limit.
const BATCH_FILES: usize = 400;
const BATCH_BYTES: u64 = 6 * 1024 * 1024 * 1024;
const RUN_TIMEOUT: i64 = 7 * 3600;

const WORKFLOW_YML: &str = r#"# Managed by XMUHub (crates/xmuhub-server/src/transfer.rs) — do not edit by hand.
name: transfer
on:
  workflow_dispatch:
    inputs:
      job:
        description: job id
        required: true
permissions:
  contents: write
concurrency:
  group: transfer
jobs:
  copy:
    runs-on: ubuntu-latest
    timeout-minutes: 350
    steps:
      - uses: actions/checkout@v4
      - name: copy files into a release
        env:
          GH_TOKEN: ${{ github.token }}
        run: python3 transfer.py "jobs/${{ inputs.job }}.json"
      - name: commit results
        run: |
          git config user.name xmuhub-transfer
          git config user.email transfer@users.noreply.github.com
          git add results
          git commit -m "results ${{ inputs.job }}"
          for i in 1 2 3; do git pull --rebase && git push && break; sleep 5; done
"#;

const TRANSFER_PY: &str = r#"# Managed by XMUHub — copies referenced files into a release of this repository.
import hashlib, json, os, sys, time, urllib.parse, urllib.request

job = json.load(open(sys.argv[1], encoding='utf8'))
repo = os.environ['GITHUB_REPOSITORY']
H = {'Authorization': 'Bearer ' + os.environ['GH_TOKEN'], 'Accept': 'application/vnd.github+json', 'User-Agent': 'xmuhub-transfer'}

def call(method, url, body=None, data=None, headers=None):
    h = dict(H, **(headers or {}))
    if body is not None:
        data = json.dumps(body).encode(); h['Content-Type'] = 'application/json'
    with urllib.request.urlopen(urllib.request.Request(url, data=data, method=method, headers=h), timeout=600) as r:
        raw = r.read()
        return json.loads(raw) if raw else None

try:
    rel = call('GET', f'https://api.github.com/repos/{repo}/releases/tags/{job["tag"]}')
except urllib.error.HTTPError:
    rel = call('POST', f'https://api.github.com/repos/{repo}/releases', {'tag_name': job['tag'], 'name': job['tag'], 'make_latest': 'false'})
have = {a['name'] for a in rel.get('assets', [])}

results = []
for it in job['items']:
    try:
        if it['name'] in have:
            results.append({'key': it['key'], 'name': it['name'], 'existing': True}); continue
        data = None
        for attempt in range(3):
            try:
                with urllib.request.urlopen(urllib.request.Request(it['url'], headers={'User-Agent': 'xmuhub-transfer'}), timeout=600) as r:
                    data = r.read()
                break
            except Exception:
                if attempt == 2: raise
                time.sleep(5)
        if len(data) != it['size']:
            raise RuntimeError(f'size {len(data)} != {it["size"]}')
        sha = hashlib.sha256(data).hexdigest()
        up = f'https://uploads.github.com/repos/{repo}/releases/{rel["id"]}/assets?name={urllib.parse.quote(it["name"])}'
        a = call('POST', up, data=data, headers={'Content-Type': 'application/octet-stream'})
        results.append({'key': it['key'], 'name': a['name'], 'asset_id': a['id'], 'sha256': sha})
    except Exception as e:
        results.append({'key': it['key'], 'error': str(e)[:300]})

os.makedirs('results', exist_ok=True)
json.dump({'tag': job['tag'], 'release_id': rel['id'], 'results': results}, open(f'results/{job["id"]}.json', 'w', encoding='utf8'), ensure_ascii=False)
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
    sha256: String,
    #[serde(default)]
    error: String,
}

pub fn current_job(hub: &Hub) -> Option<serde_json::Value> {
    let bytes = hub.meta_get(STATE_KEY).ok()??;
    let c: Current = serde_json::from_slice(&bytes).ok()?;
    Some(json!({ "id": c.id, "files": c.files, "dispatched_at": c.dispatched_at }))
}

async fn ensure_setup(gh: &GitHubBackend) -> anyhow::Result<()> {
    // Public, so Actions minutes are free and unlimited; it only holds job lists and copies
    // of files that are public anyway.
    gh.ensure_repo(REPO, false).await?;
    gh.write_file(REPO, ".github/workflows/transfer.yml", WORKFLOW_YML.as_bytes(), "update workflow").await?;
    gh.write_file(REPO, "transfer.py", TRANSFER_PY.as_bytes(), "update transfer script").await?;
    Ok(())
}

/// Applies a finished run: verify each copy against the release listing, attach it.
async fn apply(hub: &Arc<Hub>, gh: &GitHubBackend, cur: &Current, res: ResultsFile) -> anyhow::Result<(usize, usize)> {
    let Some((release_id, assets)) = gh.release_assets(REPO, &res.tag).await? else {
        anyhow::bail!("release {} missing", res.tag);
    };
    let (mut ok, mut failed) = (0, 0);
    for it in res.results {
        if !it.error.is_empty() {
            tracing::warn!(key = %it.key, "transfer failed: {}", it.error);
            failed += 1;
            continue;
        }
        let Some((asset_id, name, size, digest)) = assets.iter().find(|a| a.1 == it.name).cloned() else {
            failed += 1;
            continue;
        };
        let sha = digest.strip_prefix("sha256:").unwrap_or(&it.sha256).to_string();
        if !it.sha256.is_empty() && !digest.is_empty() && digest != format!("sha256:{}", it.sha256) {
            tracing::warn!(key = %it.key, "transfer digest mismatch");
            failed += 1;
            continue;
        }
        let expected = hub.blob_size(&it.key);
        if expected != Some(size) {
            tracing::warn!(key = %it.key, "transfer size mismatch");
            failed += 1;
            continue;
        }
        let loc = Location::GitHub { owner: gh.owner_login().to_string(), repo: REPO.into(), release_id, tag: res.tag.clone(), asset_id, name };
        let h = hub.clone();
        let key = it.key.clone();
        match tokio::task::spawn_blocking(move || h.attach_replica(&key, loc, &sha)).await {
            Ok(Ok(())) => ok += 1,
            _ => failed += 1,
        }
    }
    tracing::info!(job = %cur.id, ok, failed, "transfer run applied");
    Ok((ok, failed))
}

async fn tick(hub: &Arc<Hub>, gh: &GitHubBackend) -> anyhow::Result<()> {
    if let Some(bytes) = hub.meta_get(STATE_KEY)? {
        let cur: Current = serde_json::from_slice(&bytes)?;
        if let Some((raw, _)) = gh.read_file(REPO, &format!("results/{}.json", cur.id)).await? {
            let res: ResultsFile = serde_json::from_slice(&raw)?;
            apply(hub, gh, &cur, res).await?;
            hub.meta_put(STATE_KEY, b"")?;
        } else if now() - cur.dispatched_at > RUN_TIMEOUT {
            tracing::warn!(job = %cur.id, "transfer run timed out; its files will be retried");
            hub.meta_put(STATE_KEY, b"")?;
        }
        return Ok(());
    }
    let items = hub.unpersisted(BATCH_FILES, BATCH_BYTES);
    if items.is_empty() {
        return Ok(());
    }
    ensure_setup(gh).await?;
    let id = format!("{}", now());
    let tag = format!("t{id}");
    let job = json!({
        "id": id,
        "tag": tag,
        "items": items.iter().map(|u| {
            // Readable name + content key suffix: unique within the release, stable across retries.
            let ascii = ascii_filename(&u.basename);
            let suffix = &u.key[u.key.len().saturating_sub(10)..];
            let name = match ascii.rsplit_once('.') {
                Some((s, e)) => format!("{s}-{suffix}.{e}"),
                None => format!("{ascii}-{suffix}"),
            };
            json!({ "key": u.key, "url": u.url, "size": u.size, "name": name })
        }).collect::<Vec<_>>(),
    });
    gh.write_file(REPO, &format!("jobs/{id}.json"), serde_json::to_vec_pretty(&job)?.as_slice(), &format!("job {id}")).await?;
    // Give GitHub a moment to see the new commit on main before dispatching.
    tokio::time::sleep(Duration::from_secs(5)).await;
    gh.dispatch(REPO, WORKFLOW, json!({ "job": id })).await?;
    let cur = Current { id: id.clone(), tag, dispatched_at: now(), files: items.len() };
    hub.meta_put(STATE_KEY, &serde_json::to_vec(&cur)?)?;
    tracing::info!(job = %id, files = items.len(), "transfer run dispatched");
    Ok(())
}

pub fn spawn(hub: Arc<Hub>, gh: Arc<GitHubBackend>) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(60)).await;
        let mut every = tokio::time::interval(Duration::from_secs(5 * 60));
        loop {
            every.tick().await;
            if let Err(e) = tick(&hub, &gh).await {
                tracing::warn!("transfer: {e}");
            }
        }
    });
}

/// Runs one step right away (admin "check now").
pub async fn kick(hub: &Arc<Hub>, gh: &GitHubBackend) -> anyhow::Result<()> {
    tick(hub, gh).await
}
