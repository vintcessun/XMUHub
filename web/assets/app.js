// Shared front-end helpers: API client, session, layout, formatting, downloads.
// Plain ES modules, no build step — edit and redeploy.

export const store = {
  get(k) { try { return localStorage.getItem(k); } catch { return null; } },
  set(k, v) { try { v == null ? localStorage.removeItem(k) : localStorage.setItem(k, v); } catch { /* private mode */ } },
};

export class ApiError extends Error {
  /** `data`: the whole JSON error body (some carry more than the message, e.g. `captcha`). */
  constructor(status, message, data = null) { super(message); this.status = status; this.data = data; }
}

// API calls in flight; a page switch waits for them so the new page shows up already filled in.
let inflight = 0;
const idleWaiters = new Set();

/** JSON API call. Every non-GET carries the anti-CSRF header the server requires. */
export async function api(path, { method = 'GET', body, signal } = {}) {
  const headers = {};
  if (method !== 'GET') {
    headers['X-XMUHub'] = '1';
    // Pages now stay open across switches (see navigation below), so drop what a write may have changed.
    treeCache = null;
    metaCache = null;
  }
  inflight++;
  try { return await request(path, method, headers, body, signal); } finally {
    if (--inflight === 0) setTimeout(() => { if (!inflight) { for (const f of idleWaiters) f(); idleWaiters.clear(); } }, 0);
  }
}

/** Resolves once no API call has been running for a moment (the page has drawn its data), or after `ms`. */
function settled(ms) {
  return new Promise((resolve) => {
    const done = () => { clearTimeout(t); idleWaiters.delete(done); resolve(); };
    const t = setTimeout(done, ms);
    setTimeout(() => { if (inflight) idleWaiters.add(done); else done(); }, 0);
  });
}

async function request(path, method, headers, body, signal) {
  if (body !== undefined) headers['Content-Type'] = 'application/json';
  let res;
  try {
    res = await fetch(`/api${path}`, { method, headers, body: body === undefined ? undefined : JSON.stringify(body), signal, credentials: 'same-origin' });
  } catch (e) {
    if (e.name === 'AbortError') throw e;
    throw new ApiError(0, '网络连接失败，请稍后重试');
  }
  const text = await res.text();
  let data = null;
  try { data = text ? JSON.parse(text) : null; } catch { /* non-JSON */ }
  if (!res.ok) throw new ApiError(res.status, (data && data.error) || `请求失败（HTTP ${res.status}）`, data);
  return data;
}

let meCache = null;
/** The signed-in user or null. */
export function me() {
  if (!meCache) meCache = api('/me').then((r) => r.user).catch(() => null);
  return meCache;
}
export function forgetMe() {
  meCache = null;
  try { sessionStorage.removeItem('ludao.top'); } catch { /* storage blocked */ }
}

// A page can now stay open for a long time, so these are refreshed every few minutes.
const STALE = 5 * 60_000;
let metaCache = null;
let metaAt = 0;
export function meta() {
  if (!metaCache || Date.now() - metaAt > STALE) { metaCache = api('/meta'); metaAt = Date.now(); }
  return metaCache;
}

let treeCache = null;
let treeAt = 0;
/** The whole category tree: { list, byId, children(id) }. */
const zh = new Intl.Collator('zh-CN', { numeric: true });
/**
 * Sibling order (in place): numbered groups keep the scheme's order (A1 思政, A2 数学 …);
 * unnumbered colleges and courses follow them by 拼音 initial, so a long list can be scanned.
 * Levels (I-1, 上 …) keep the server's order; a `sort` set by hand wins over both.
 */
export function sortNodes(list) {
  const key = (n) => (n.kind === 'level' ? 0 : n.code ? 1 : 2);
  const idx = new Map(list.map((n, i) => [n, i]));
  const pos = (n) => n.sort || Infinity; // an order set by hand comes first
  return list.sort((a, b) => (pos(a) === pos(b) ? 0 : pos(a) - pos(b)) || key(a) - key(b)
    || (key(a) === 1 ? zh.compare(a.code, b.code) : key(a) === 2 ? zh.compare(a.name, b.name) : 0)
    || idx.get(a) - idx.get(b));
}

export function tree() {
  if (!treeCache || Date.now() - treeAt > STALE) {
    treeAt = Date.now();
    treeCache = api('/tree').then((list) => {
      const byId = new Map(list.map((n) => [n.id, n]));
      const kids = new Map();
      for (const n of list) {
        const k = n.parent ?? 0;
        if (!kids.has(k)) kids.set(k, []);
        kids.get(k).push(n);
      }
      // The top-level sections keep the order of the scheme (校选课 · 公共课 · 专业课 …).
      for (const [p, k] of kids) if (p) sortNodes(k);
      return { list, byId, children: (id) => kids.get(id ?? 0) || [] };
    });
  }
  return treeCache;
}

/** Site slogan, shown big on the home page. */
export const SLOGAN = '让每一份资料都被需要它的人找到';
/** User group shown on the feedback page, e.g. 'QQ 群 123456789'; empty hides it. */
export const REPO_URL = '{{site.repo}}';

export const COMMUNITY = '{{site.community}}';
const ICP = '{{site.icp}}';
/** The site's name, for page titles. */
export const SITE_NAME = '{{site.name}}';

export const LEVELS = ['访客', '贡献者', '可信贡献者', '审核员', '管理员'];

// ---------------------------------------------------------------- formatting

export function esc(s) {
  return String(s ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
}

/** Plain text → HTML with its links clickable: web addresses, and 「GitHub owner/repo」 (how
 * imported files name their source repo). The text is escaped first, so nothing else in it
 * can become markup; links open in a new tab and pass no referrer. */
export function linkify(text) {
  const a = (href, label) => `<a href="${href}" target="_blank" rel="noopener noreferrer nofollow">${label}</a>`;
  return esc(text)
    // Addresses are ASCII: the first CJK character (a 。 or the next sentence) ends one.
    .replace(/https?:\/\/[^\s<\u0080-\uffff]+/g, (u) => {
      // Trailing punctuation (and escaped quotes) belongs to the sentence, not the address.
      const [, url, tail] = u.match(/^(.*?)((?:&quot;|&#39;|[.,;:!?)）。，、；：！？])*)$/);
      return a(url, url) + tail;
    })
    .replace(/(^|[^\w./-])GitHub ([A-Za-z0-9_.-]+)\/([A-Za-z0-9_-][A-Za-z0-9_.-]*)/g,
      (_, pre, owner, repo) => `${pre}${a(`https://github.com/${owner}/${repo}`, `GitHub ${owner}/${repo}`)}`);
}

export function fmtSize(n) {
  if (n == null) return '';
  const u = ['B', 'KB', 'MB', 'GB'];
  let i = 0;
  while (n >= 1024 && i < u.length - 1) { n /= 1024; i++; }
  return `${n >= 100 || i === 0 ? Math.round(n) : n.toFixed(1)} ${u[i]}`;
}

export function fmtDate(sec, withTime = false) {
  if (!sec) return '';
  const d = new Date(sec * 1000);
  const p = (x) => String(x).padStart(2, '0');
  const day = `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
  return withTime ? `${day} ${p(d.getHours())}:${p(d.getMinutes())}` : day;
}

export function ago(sec) {
  const s = Date.now() / 1000 - sec;
  if (s < 60) return '刚刚';
  if (s < 3600) return `${Math.floor(s / 60)} 分钟前`;
  if (s < 86400) return `${Math.floor(s / 3600)} 小时前`;
  if (s < 86400 * 30) return `${Math.floor(s / 86400)} 天前`;
  return fmtDate(sec);
}

export const STATUS = { pending: '待审核', published: '已发布', rejected: '未通过', removed: '已下架', restricted: '仅内部' };

export function statusBadge(r) {
  const bits = [];
  if (r.status !== 'published') bits.push(`<span class="badge ${esc(r.status)}">${STATUS[r.status] || esc(r.status)}</span>`);
  else if (r.needs_review) bits.push('<span class="badge pending">待复核</span>');
  if (r.uncertain) bits.push('<span class="badge uncertain">待核实</span>');
  return bits.join('');
}

export function pathText(path) {
  return (path || []).filter((p) => p.kind !== 'section').map((p) => p.name).join(' / ');
}

/** Display name of a node: levels ("上", "I-1") read badly alone, so use their label. */
export function nodeTitle(n) {
  return n.kind === 'level' && n.label && n.label !== n.name ? n.label : n.name;
}

export function stars(avg) {
  const full = Math.round(avg);
  return `<span class="stars" aria-label="${avg} 星">${'★'.repeat(full)}<i>${'★'.repeat(5 - full)}</i></span>`;
}

export function resourceItem(r, { showNode = true } = {}) {
  const e = r.ext || 'file';
  const bits = [
    `<span class="tag t-${esc(r.tag.code)}">${esc(r.tag.label)}</span>`,
    showNode ? `<a href="/n/${r.node.id}">${esc(nodeTitle(r.node))}</a>` : '',
    `<span>${fmtSize(r.size)}</span>`,
    `<span>${r.downloads} 次下载</span>`,
    r.rating && r.rating.count ? `<span title="${r.rating.count} 人评分">${stars(r.rating.avg)} ${r.rating.avg}</span>` : '',
    r.major ? `<span class="badge" title="适用专业">${esc(r.major)}</span>` : '',
    statusBadge(r),
  ].filter(Boolean);
  // First-page thumbnail when one has been made (see server thumbs.rs); else the type badge.
  const icon = r.thumb && r.thumb.length
    ? `<img class="thumb" src="${esc(r.thumb[0])}" data-alts="${esc(r.thumb.slice(1).join(' '))}" data-ext="${esc(e.slice(0, 4))}" loading="lazy" alt="" title="点击预览">`
    : `<div class="ficon ${esc(e)}">${esc(e.slice(0, 4))}</div>`;
  return `<div class="item">
    ${icon}
    <div class="body"><a class="title" href="/r/${r.id}">${esc(r.title)}</a>
      ${r.subtitle ? `<div class="subtitle">${esc(r.subtitle)}</div>` : ''}
      ${r.note ? `<div class="note">${linkify(r.note)}</div>` : ''}
      <div class="meta">${bits.join('')}</div></div>
    <button class="btn sm qpv" data-qpv="${r.id}" type="button" title="不用点进去，直接看内容">预览</button>
  </div>`;
}

/** Sends one upload part to its storage target (Worker or this server's relay). */
export function sendPart(target, blob, onProgress) {
  return new Promise((resolve, reject) => {
    const xhr = new XMLHttpRequest();
    xhr.open(target.method, target.url);
    for (const [k, v] of target.headers) xhr.setRequestHeader(k, v);
    // Same-origin relay requires the anti-CSRF header; the cross-origin Worker must not get it.
    if (target.url.startsWith('/')) xhr.setRequestHeader('X-XMUHub', '1');
    xhr.upload.onprogress = (e) => onProgress(e.loaded);
    xhr.onload = () => {
      let body = null;
      try { body = JSON.parse(xhr.responseText); } catch { /* ignore */ }
      if (xhr.status >= 200 && xhr.status < 300) resolve(body || {});
      else reject(new Error((body && (body.error || body.message)) || `上传失败（HTTP ${xhr.status}）`));
    };
    xhr.onerror = () => reject(new Error('网络中断'));
    xhr.send(blob);
  });
}

/** Profile picture (mirror URLs; the next one is tried on error), else the nickname's
 * first character on a colour picked from the name. */
export function avatar(urls, name, size = 28) {
  if (urls && urls.length) {
    return `<img class="avatar" style="--s:${size}px" src="${esc(urls[0])}" data-alts="${esc(urls.slice(1).join(' '))}" data-name="${esc(name || '')}" alt="" loading="lazy">`;
  }
  return letterAvatar(name, size);
}
function letterAvatar(name, size) {
  const chars = [...(name || '?')];
  const hue = chars.reduce((h, c) => (h * 31 + c.codePointAt(0)) % 360, 7);
  return `<span class="avatar" style="--s:${size}px;--h:${hue}" aria-hidden="true">${esc(chars[0] || '?')}</span>`;
}

/** Study levels of a course (set on it or inherited from its college / group). */
export const STUDY_LEVELS = { 1: '本科', 2: '研究生', 3: '本研' };

/** 本科 / 研究生 badge for a node (nothing when no level is set). */
export function levelBadge(n) {
  const l = STUDY_LEVELS[n.level];
  return l ? `<span class="lvl lvl-${n.level}">${l}</span>` : '';
}

export function nodeCard(n, path) {
  const sub = path ? pathText(path) : (n.code || '');
  return `<a class="card node-card" href="/n/${n.id}">
    <h3>${esc(nodeTitle(n))}${levelBadge(n)}</h3>
    <div class="meta">${sub ? `<span>${esc(sub)}</span>` : ''}<span><b class="count">${n.count}</b> 份</span></div>
  </a>`;
}

export function toast(msg, bad = false) {
  const el = document.createElement('div');
  el.className = 'toast' + (bad ? ' bad' : '');
  el.textContent = msg;
  document.body.appendChild(el);
  setTimeout(() => el.remove(), bad ? 4500 : 2500);
}

/** The current query string. Read on each call: the address changes without a reload (see navigation below). */
export const qs = { get: (k) => new URLSearchParams(location.search).get(k) };

/** Numeric id from a clean URL such as /n/12 or /r/34. */
export function pathId() {
  const m = /\/(\d+)\/?$/.exec(location.pathname);
  return m ? Number(m[1]) : null;
}

export function $(sel, root = document) { return root.querySelector(sel); }

export function loginUrl() {
  return `/login?next=${encodeURIComponent(location.pathname + location.search)}`;
}

// ---------------------------------------------------------------- layout

export async function layout(active) {
  const top = document.getElementById('top');
  const q = qs.get('q') || '';
  top.className = 'top';
  top.innerHTML = `<div class="wrap">
    <a class="brand" href="/"><img class="logo" src="{{site.logo}}" alt=""><span><b>{{site.name}}</b><small>{{site.subtitle}}</small></span></a>
    ${active === 'home' || active === 'search' ? '<span class="grow"></span>' : `<form action="/search" role="search"><input name="q" value="${esc(q)}" placeholder="搜索课程名称…" aria-label="搜索"></form>`}
    <nav class="nav">
      <a href="/browse" data-k="browse">分类</a>
      <a href="/help" data-k="help">教程</a>
      <a href="${REPO_URL}" class="gh" rel="noopener" target="_blank" title="本站完全开源，欢迎 Star 和参与开发">⭐ 开源</a>
      <a href="/admin" data-k="admin" hidden>审核</a>
      <a href="/notices" data-k="notices" id="nav-bell" class="bell" title="站内提醒" hidden>提醒<span class="count" hidden></span></a>
      <a href="/me" data-k="me" id="nav-me">登录</a>
      <a href="/upload" data-k="upload" class="up">上传</a>
    </nav></div>`;
  top.querySelectorAll('.nav a').forEach((a) => { if (a.dataset.k === active && a.dataset.k !== 'upload') a.classList.add('on'); });
  const foot = document.getElementById('foot');
  foot.className = 'foot';
  foot.innerHTML = `<div class="wrap">
    <span>{{site.footer}}</span>
    <span><a href="/help">使用教程</a> · <a href="/about">使用须知</a> · <a href="/feedback">意见反馈</a>${COMMUNITY ? `（${esc(COMMUNITY)}）` : ''} · <a href="/collections">收藏夹</a> · <a href="/links">站外资源</a> · <a href="/missing">缺资料的课程</a> · <a href="/stats">统计</a> · <a href="${REPO_URL}" rel="noopener">源代码（AGPL-3.0）</a> · 资料由用户上传，仅供个人学习交流</span>
    <span>本站服务器不存储任何资料文件。文件的上传、存储及下载由第三方服务提供商（GitHub 及国内公共加速镜像）完成，本站仅提供上传辅助、资源信息展示及第三方链接索引服务。<a href="/about#hosting">文件托管说明</a> · <a href="/about">免责声明</a> · <a href="/privacy">隐私政策</a></span>
    <span>如认为资料侵犯了您的著作权或其他合法权益，请通过资料页「投诉 / 申请下架」或<a href="/feedback">意见反馈</a>联系我们（无需注册），核实后我们会在 48 小时内下架。<a href="/about#copyright">版权声明与侵权投诉</a></span>
    ${ICP ? `<span><a href="https://beian.miit.gov.cn/" target="_blank" rel="noopener">${esc(ICP)}</a></span>` : ''}</div>`;
  feedbackButton(active !== 'feedback');
  topButton();
  announcement();
  const user = await me();
  const navMe = top.querySelector('#nav-me');
  if (user) {
    navMe.innerHTML = `${avatar(user.avatar, user.nickname, 22)}<span>${esc(user.nickname)}</span>`;
    if (user.level >= 3) top.querySelector('[data-k="admin"]').hidden = false;
    top.querySelector('#nav-bell').hidden = false;
    setBell(user.unread || 0);
  } else {
    navMe.href = loginUrl();
  }
  // The next page paints this header/footer before its scripts run, so switching pages
  // doesn't flash an empty bar (see the inline script after <header id="top">).
  try {
    sessionStorage.setItem('ludao.top', top.innerHTML);
    sessionStorage.setItem('ludao.foot', foot.innerHTML);
  } catch { /* storage blocked */ }
  // Prefetching /me and /upload on hover: see assets/speculation-rules.json (sent by the
  // server in a Speculation-Rules header, since the CSP doesn't allow inline rules).
  return user;
}

/** The header's 提醒 link: its unread count, if any. */
export function setBell(n) {
  const c = document.querySelector('#nav-bell .count');
  if (!c) return;
  c.hidden = !n;
  c.textContent = n > 99 ? '99+' : String(n || '');
  document.querySelector('#nav-bell').title = n ? `${n} 条未读提醒` : '站内提醒';
}

// ---------------------------------------------------------------- 收藏夹

/** Puts a file on the viewer's 收藏夹 lists (a dialog with a checkbox per list and a quick
 * 「新建」). Resolves once closed. */
export async function favoriteDialog(resourceId) {
  const [lists, inIds] = await Promise.all([api('/collections'), api(`/resources/${resourceId}/collections`)]);
  const inSet = new Set(inIds);
  const body = modal('收藏到…');
  const render = () => {
    body.innerHTML = `${lists.length ? `<div class="list">${lists.map((c) => `<label class="item" style="cursor:pointer"><input type="checkbox" data-c="${c.id}"${inSet.has(c.id) ? ' checked' : ''}>
        <div class="body"><b>${esc(c.title)}</b> <span class="small faint">${c.count} 份${c.status === 'public' ? ' · 已公开' : ''}</span></div></label>`).join('')}</div>` : '<p class="small muted">还没有收藏夹，新建一个吧。</p>'}
      <div class="row" style="margin-top:12px"><input class="input" id="fvnew" maxlength="40" placeholder="新收藏夹的名字，如「高数期末」" style="max-width:260px"><button class="btn sm" id="fvadd" type="button">新建并收藏</button>
      <span class="grow"></span><a class="small" href="/collections">管理收藏夹</a></div>`;
    body.querySelector('#fvadd').onclick = async () => {
      const title = body.querySelector('#fvnew').value.trim() || '我的收藏';
      try {
        const c = await api('/collections', { method: 'POST', body: { title } });
        await api(`/collections/${c.id}/items/${resourceId}`, { method: 'PUT', body: { on: true } });
        lists.unshift({ ...c, count: 1 });
        inSet.add(c.id);
        toast(`已收藏到「${c.title}」`);
        render();
      } catch (e) { toast(e.message, true); }
    };
  };
  body.onchange = async (e) => {
    const box = e.target.closest('[data-c]');
    if (!box) return;
    const id = Number(box.dataset.c);
    try {
      await api(`/collections/${id}/items/${resourceId}`, { method: 'PUT', body: { on: box.checked } });
      const c = lists.find((x) => x.id === id);
      if (box.checked) { inSet.add(id); c.count++; } else { inSet.delete(id); c.count--; }
      toast(box.checked ? `已收藏到「${c.title}」` : `已从「${c.title}」拿出`);
    } catch (err) { box.checked = !box.checked; toast(err.message, true); }
  };
  render();
  return inSet;
}

// ---------------------------------------------------------------- downloads

function saveBlob(blob, name) {
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = name;
  document.body.appendChild(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 60_000);
}

let hasherLib = null;
/** hash-wasm (vendored), for SHA-256 computed chunk by chunk while a part streams in. */
function loadHasher() {
  if (window.hashwasm?.createSHA256) return Promise.resolve(window.hashwasm);
  if (!hasherLib) {
    hasherLib = new Promise((resolve, reject) => {
      const s = document.createElement('script');
      s.src = '/vendor/sha256.umd.min.js';
      s.onload = () => (window.hashwasm?.createSHA256 ? resolve(window.hashwasm) : reject(new Error('加载校验组件失败')));
      s.onerror = () => { hasherLib = null; reject(new Error('加载校验组件失败')); };
      document.head.appendChild(s);
    });
  }
  return hasherLib;
}

/** A part whose bytes don't match the SHA-256 in the download plan. */
export class IntegrityError extends Error {
  constructor() { super('文件校验失败（内容与记录不符）'); this.integrity = true; }
}

const hex = (buf) => [...new Uint8Array(buf)].map((b) => b.toString(16).padStart(2, '0')).join('');

/**
 * Fetches one part, trying each URL in turn. A mirror that is reachable but crawling
 * (under ~150 KB/s after a few seconds) is abandoned for the next one, and so is one whose
 * bytes don't match the part's SHA-256 from the download plan.
 */
export async function fetchPart(part, urls, onBytes, alive = () => true) {
  const want = String(part.sha256 || '').toLowerCase();
  // Hashed as the bytes arrive (no second copy); WebCrypto on the whole part if hash-wasm can't load.
  const hw = want ? await loadHasher().catch(() => null) : null;
  let lastErr;
  for (const [i, url] of urls.entries()) {
    if (!alive()) throw new Error('已取消');
    const hasNext = i < urls.length - 1;
    const ctl = new AbortController();
    let timer;
    try {
      const resetTimeout = (ms = 30_000) => { clearTimeout(timer); timer = setTimeout(() => ctl.abort(), ms); };
      // A mirror that doesn't even answer within 8 s is skipped when there's another to try.
      resetTimeout(hasNext ? 8_000 : 30_000);
      const res = await fetch(url, { signal: ctl.signal, mode: 'cors', credentials: url.startsWith('/') ? 'same-origin' : 'omit', referrerPolicy: 'no-referrer' });
      if (!res.ok || !res.body) throw new Error(`HTTP ${res.status}`);
      const reader = res.body.getReader();
      const hash = hw ? await hw.createSHA256() : null;
      hash?.init();
      const chunks = [];
      const t0 = performance.now();
      let got = 0;
      for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        chunks.push(value);
        hash?.update(value);
        got += value.length;
        onBytes(got);
        // The preview was closed or the page switched: stop downloading.
        if (!alive()) { ctl.abort(); throw new Error('已取消'); }
        resetTimeout();
        const secs = (performance.now() - t0) / 1000;
        if (hasNext && secs > 6 && got / secs < 150 * 1024) { ctl.abort(); throw new Error('镜像太慢'); }
      }
      clearTimeout(timer);
      if (got !== part.size) throw new Error('大小不符');
      const blob = new Blob(chunks);
      // A mirror serving other bytes (tampered, or an error page of the right size) counts as failed.
      if (want) {
        const got256 = hash ? hash.digest('hex') : hex(await crypto.subtle.digest('SHA-256', await blob.arrayBuffer()));
        if (got256 !== want) throw new IntegrityError();
      }
      return blob;
    } catch (e) {
      lastErr = e;
      onBytes(0);
    } finally {
      clearTimeout(timer);
    }
  }
  throw lastErr || new Error('没有可用的下载地址');
}

/** Parts of one file sent or fetched side by side: one connection carries ~1 MB/s. */
export const PARTS_AT_ONCE = 3;

/** `work(item, index)` for every item, at most `limit` at a time; results in the items' order. */
export async function inParallel(items, limit, work) {
  const out = new Array(items.length);
  let next = 0;
  const lane = async () => {
    while (next < items.length) {
      const i = next++;
      out[i] = await work(items[i], i);
    }
  };
  await Promise.all(Array.from({ length: Math.min(limit, items.length) }, lane));
  return out;
}

/**
 * Downloads a resource. Single files: try a cross-origin fetch through the fastest mirror
 * (keeps the Chinese filename); most mirrors send no CORS headers, in which case we navigate
 * to that same fastest mirror (full speed, ASCII filename). Multi-part files must be fetched
 * and joined in the browser, so every mirror is tried for each part.
 */
export async function downloadResource(id, onProgress = () => {}, { batch = false } = {}) {
  const plan = await api(`/resources/${id}/download`);
  const total = plan.size || 1;
  const single = plan.parts.length === 1;
  const got = plan.parts.map(() => 0);
  try {
    // One click, one file: the first mirror, else the browser opens it itself (below). A batch
    // can't hand files to the browser that way, so it tries every mirror that allows fetching.
    const blobs = await inParallel(plan.parts, PARTS_AT_ONCE, (part, i) => {
      const urls = single && !batch ? part.urls.slice(0, 1) : part.urls;
      return fetchPart(part, urls, (n) => { got[i] = n; onProgress(Math.min(1, got.reduce((a, b) => a + b, 0) / total)); });
    });
    saveBlob(new Blob(blobs, { type: plan.mime || 'application/octet-stream' }), plan.filename);
    return { ok: true };
  } catch (e) {
    // Usually the fetch failed only because the mirror sends no CORS headers: then the browser
    // downloads from it directly. Never send the user to a mirror whose bytes just failed the
    // checksum; use the plain GitHub address (the last one) instead.
    // In a batch the next file would cancel a page navigation (and the CSP blocks mirror
    // frames), so the caller lists what failed for the user to click one by one.
    if (single && !batch) {
      const urls = plan.parts[0].urls;
      location.href = e && e.integrity ? urls[urls.length - 1] : urls[0];
      return { ok: true, fallback: true };
    }
    return { ok: false, plan, error: e };
  }
}

// ---------------------------------------------------------------- modal, password toggle

/** Opens a modal dialog and returns its body element (with `.close()`). */
export function modal(title, { wide = false } = {}) {
  const wrap = document.createElement('div');
  wrap.className = 'modal';
  wrap.innerHTML = `<div class="modal-box${wide ? ' wide' : ''}" role="dialog" aria-modal="true">
    <div class="modal-head"><b>${esc(title)}</b><span class="grow"></span><button class="btn sm" data-close type="button">关闭</button></div>
    <div class="modal-body"></div></div>`;
  const onKey = (e) => { if (e.key === 'Escape') close(); };
  function close() { wrap.remove(); document.removeEventListener('keydown', onKey); }
  wrap.onclick = (e) => { if (e.target === wrap || e.target.closest('[data-close]')) close(); };
  document.addEventListener('keydown', onKey);
  document.body.appendChild(wrap);
  const body = wrap.querySelector('.modal-body');
  body.close = close;
  return body;
}

/** Adds a 显示/隐藏 button to each password input. */
export function pwToggle(...inputs) {
  for (const input of inputs) {
    if (!input || input.dataset.pw) continue;
    input.dataset.pw = '1';
    const box = document.createElement('span');
    box.className = 'pwbox';
    input.replaceWith(box);
    const b = document.createElement('button');
    b.type = 'button';
    b.className = 'pweye';
    b.textContent = '显示';
    b.onclick = () => {
      const show = input.type === 'password';
      input.type = show ? 'text' : 'password';
      b.textContent = show ? '隐藏' : '显示';
    };
    box.append(input, b);
  }
}

// ---------------------------------------------------------------- preview

/** Renders a preview of resource `id` into `box` (code loaded on first use). */
export async function preview(id, box) {
  const m = await import('./preview.js');
  return m.preview(id, box);
}

// ---------------------------------------------------------------- node picker

/** Picks a category from the whole tree in a dialog (expand a section / college, or filter
 * by name), without leaving the page. Resolves to { node, path } (path = its ancestors) or
 * null. Sections only expand: files go into a college, group or course. */
/**
 * ↑ / ↓ (also the numeric keypad's arrows with Num Lock off) move a highlight through the
 * `items` of `list` while focus stays in `input`; the list scrolls along. Enter acts on the
 * highlighted one (`act`, default: click it). When the list is redrawn the highlight starts
 * over, unless `act` hands back the element to keep highlighted.
 */
export function arrowKeys(input, list, items, act = (el) => el.click()) {
  let cur = null;
  const all = () => [...list.querySelectorAll(items)].filter((el) => el.offsetParent !== null);
  const mark = (el) => {
    list.querySelectorAll('.on').forEach((x) => x.classList.remove('on'));
    cur = el;
    if (el) { el.classList.add('on'); el.scrollIntoView({ block: 'nearest' }); }
  };
  input.addEventListener('keydown', (e) => {
    const els = all();
    const at = els.indexOf(cur);
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      if (!els.length) return;
      e.preventDefault();
      mark(els[e.key === 'ArrowDown' ? (at + 1) % els.length : (at <= 0 ? els.length : at) - 1]);
    } else if (e.key === 'Enter' && at >= 0) {
      e.preventDefault();
      mark(act(cur) || null);
    }
  });
}

export async function pickFromTree(title = '在分类树里选') {
  const t = await tree();
  return new Promise((resolve) => {
    const body = modal(title, { wide: true });
    body.innerHTML = `<input class="input" placeholder="筛选：课程名、学院或俗称" autocomplete="off" style="margin-bottom:10px">
      <ul class="tree picktree"></ul>`;
    const input = body.querySelector('input');
    const ul = body.querySelector('ul');
    const open = new Set(t.children(0).map((n) => n.id));
    let filter = '';
    let done = false;
    const finish = (v) => { if (done) return; done = true; body.close(); resolve(v); };
    const observer = new MutationObserver(() => { if (!body.isConnected) { observer.disconnect(); if (!done) { done = true; resolve(null); } } });
    observer.observe(document.body, { childList: true });
    const render = () => {
      let shown = null;
      if (filter) {
        shown = new Set();
        const f = filter.toLowerCase();
        for (const n of t.list) {
          if (![n.name, n.label, n.code, ...(n.aliases || [])].some((s) => s && s.toLowerCase().includes(f))) continue;
          for (let x = n; x; x = t.byId.get(x.parent)) shown.add(x.id);
        }
      }
      const li = (n) => {
        const kids = t.children(n.id).filter((k) => !shown || shown.has(k.id));
        const isOpen = shown ? true : open.has(n.id);
        return `<li><div class="row-n${kids.length ? ' has-kids' : ''}" data-t="${n.id}">
            ${kids.length ? `<button class="tw" type="button" data-t="${n.id}">${isOpen ? '▾' : '▸'}</button>` : '<span class="tw"></span>'}
            <a class="n${n.count ? '' : ' zero'}" href="#" data-t="${n.id}">${esc(n.name)}</a>${n.own_level ? levelBadge(n) : ''}
            ${n.status === 'pending' ? '<span class="badge pending">待确认</span>' : ''}
            ${n.kind !== 'section' ? `<button class="btn sm pick${kids.length ? '' : ' primary'}" type="button" data-pick="${n.id}">选这里</button>` : ''}
            <span class="c">${n.count || ''}</span></div>
          ${kids.length && isOpen ? `<ul>${kids.map(li).join('')}</ul>` : ''}</li>`;
      };
      ul.innerHTML = t.children(0).filter((n) => !shown || shown.has(n.id)).map(li).join('') || '<li class="empty">没有匹配的分类</li>';
    };
    ul.onclick = (e) => {
      const p = e.target.closest('[data-pick]');
      const hit = p || e.target.closest('[data-t]');
      if (!hit) return;
      e.preventDefault();
      const n = t.byId.get(Number(p ? p.dataset.pick : hit.dataset.t));
      if (!n) return;
      // A leaf (course) is picked by its name; anything with children expands instead.
      if (p || (!t.children(n.id).length && n.kind !== 'section')) {
        const path = [];
        for (let x = t.byId.get(n.parent); x; x = t.byId.get(x.parent)) path.unshift(x);
        finish({ node: n, path });
        return;
      }
      if (filter) return;
      open.has(n.id) ? open.delete(n.id) : open.add(n.id);
      render();
    };
    input.oninput = () => { filter = input.value.trim(); render(); };
    // ↑ ↓ through the rows, Enter = click the name (a course is picked, a college opens);
    // the list is redrawn when a college opens, so the highlight moves back to its row.
    arrowKeys(input, ul, '.row-n', (row) => {
      row.querySelector('a.n').click();
      return ul.isConnected ? ul.querySelector(`.row-n[data-t="${row.dataset.t}"]`) : null;
    });
    render();
    input.focus();
  });
}

/** Moves resources after asking for the target; resolves to the number moved (0 if cancelled). */
export async function moveResources(ids) {
  if (!ids.length) { toast('先勾选资料', true); return 0; }
  // The whole tree (expand a college, or filter by name): same-named courses of different
  // colleges are told apart by where they sit.
  const picked = await pickFromTree(ids.length > 1 ? `把 ${ids.length} 份资料移动到…` : '改到哪门课程');
  if (!picked) return 0;
  const n = picked.node;
  try {
    const r = await api('/resources/move', { method: 'POST', body: { ids, node: n.id } });
    toast(`已移动 ${r.moved} 份到「${n.name}」`);
    return r.moved;
  } catch (e) { toast(e.message, true); return 0; }
}

// ---------------------------------------------------------------- 合集

/** 「来源 · 年份」 of a 合集, when given. */
export const seriesAbout = (s) => [s.source, s.year].filter(Boolean).map(esc).join(' · ');

const byName = (a, b) => a.title.localeCompare(b.title, 'zh-CN', { numeric: true });

/**
 * 整理成合集 / 修改合集: name the set and put its files in order. Anyone signed in may propose;
 * a reviewer who didn't propose it approves (admins' apply at once). `series` is the live
 * 合集 being changed, or null for a new one made of the files `pick` (ids).
 * Resolves true when the change is already live.
 */
export async function seriesEditor(node, series = null, pick = []) {
  const admin = ((await me()) || {}).level >= 4;
  let d;
  try { d = await api(`/nodes/${node}`); } catch (e) { toast(e.message, true); return false; }
  const byId = new Map(d.resources.map((r) => [r.id, r]));
  const taken = new Map();
  for (const s of d.series || []) if (!series || s.id !== series.id) for (const i of s.items) taken.set(i.id, s.title);
  let items = series ? series.items.map((i) => i.id) : pick.filter((id) => byId.has(id)).sort((a, b) => byName(byId.get(a), byId.get(b)));
  const body = modal(series ? `修改合集「${series.title}」` : '整理成合集', { wide: true });
  return new Promise((resolve) => {
    body.innerHTML = `<p class="small muted" style="margin-top:0">把这门课里连续的资料（第1讲…第12讲、历年期末…）按顺序排好，课程页上会合成一行，资料页可以直接切到上一份 / 下一份。
      一份资料只能在一个合集里。${admin ? '你是管理员，提交后直接生效。' : '提交后由审核员审核，通过后显示。'}</p>
      <div class="fields-3">
        <label class="field"><span>合集名称 <em>*</em></span><input class="input" id="s_title" maxlength="40" value="${esc(series ? series.title : '')}" placeholder="如：数据结构课件（第1–12章）"></label>
        <label class="field"><span>来源（选填）</span><input class="input" id="s_source" maxlength="60" value="${esc(series ? series.source : '')}" placeholder="如：张老师班、学长整理"></label>
        <label class="field"><span>年份（选填）</span><input class="input" id="s_year" maxlength="20" value="${esc(series ? series.year : '')}" placeholder="如：2024 或 2019–2024"></label>
      </div>
      <div class="row small" style="margin:10px 0 6px"><b id="s_count"></b><span class="grow"></span><button class="btn sm" id="s_sort" type="button">按名称排序</button></div>
      <ol class="series-edit" id="s_list"></ol>
      <details class="series-add"><summary class="small">添加这门课的其他资料</summary>
        <input class="input" id="s_q" placeholder="按名称筛选" style="margin:8px 0">
        <div id="s_more"></div></details>
      <div class="row" style="margin-top:14px"><button class="btn primary" id="s_go" type="button">${admin ? '保存' : '提交审核'}</button>
        ${series ? '<span class="grow"></span><button class="btn danger sm" id="s_close" type="button">解散合集</button>' : ''}</div>`;
    const title = (id) => esc(byId.get(id)?.title || `资料 #${id}`);
    const drawList = () => {
      body.querySelector('#s_count').textContent = `顺序（${items.length} 份）`;
      body.querySelector('#s_list').innerHTML = items.map((id, i) => `<li><span class="grow">${title(id)}</span>
        <button class="btn sm" data-up="${i}" type="button"${i ? '' : ' disabled'} aria-label="上移">↑</button><button class="btn sm" data-down="${i}" type="button"${i < items.length - 1 ? '' : ' disabled'} aria-label="下移">↓</button><button class="btn sm" data-rm="${i}" type="button">移出</button></li>`).join('')
        || '<li class="faint">还没有资料，从下面添加</li>';
      drawMore();
    };
    const drawMore = () => {
      const q = body.querySelector('#s_q').value.trim().toLowerCase();
      const rest = [...byId.values()].filter((r) => !items.includes(r.id) && (!q || r.title.toLowerCase().includes(q))).sort(byName);
      body.querySelector('#s_more').innerHTML = rest.map((r) => `<div class="row small series-cand"><span class="grow">${esc(r.title)}${taken.has(r.id) ? ` <span class="faint">（已在合集「${esc(taken.get(r.id))}」里）</span>` : ''}</span>
        <button class="btn sm" data-add="${r.id}" type="button"${taken.has(r.id) ? ' disabled' : ''}>加入</button></div>`).join('') || '<p class="small faint">没有其他资料了</p>';
    };
    body.querySelector('#s_q').oninput = drawMore;
    body.querySelector('#s_sort').onclick = () => { items.sort((a, b) => byName(byId.get(a) || { title: '' }, byId.get(b) || { title: '' })); drawList(); };
    body.onclick = (e) => {
      const b = e.target.closest('button');
      if (!b) return;
      const i = Number(b.dataset.up ?? b.dataset.down ?? b.dataset.rm);
      if (b.dataset.up) [items[i - 1], items[i]] = [items[i], items[i - 1]];
      else if (b.dataset.down) [items[i + 1], items[i]] = [items[i], items[i + 1]];
      else if (b.dataset.rm) items.splice(i, 1);
      else if (b.dataset.add) items.push(Number(b.dataset.add));
      else return;
      drawList();
    };
    const send = async (list) => {
      try {
        const s = await api(series ? `/series/${series.id}` : '/series', { method: series ? 'PUT' : 'POST', body: { node, title: body.querySelector('#s_title').value, source: body.querySelector('#s_source').value, year: body.querySelector('#s_year').value, items: list } });
        const live = !s.draft;
        toast(live ? (list.length ? '合集已更新' : '合集已解散') : '已提交，审核通过后显示');
        body.close();
        resolve(live);
      } catch (err) { toast(err.message, true); }
    };
    body.querySelector('#s_go').onclick = () => send(items);
    if (series) body.querySelector('#s_close').onclick = () => { if (confirm('解散这个合集？资料本身不受影响。')) send([]); };
    drawList();
  });
}

// ---------------------------------------------------------------- announcement

const ANN_KEY = 'xmuhub.announce.closed';
/** The admins' site-wide notice under the header, until the reader closes this version of it. */
function announcement() {
  meta().then((m) => {
    const a = m.announcement;
    document.querySelector('.announce')?.remove();
    if (!a || !a.text || store.get(ANN_KEY) === String(a.at)) return;
    const bar = document.createElement('div');
    bar.className = 'announce';
    bar.setAttribute('role', 'note');
    bar.innerHTML = `<div class="wrap"><b>公告</b><span class="grow">${linkify(a.text)}</span><button type="button" aria-label="关闭公告" title="关闭">×</button></div>`;
    bar.querySelector('button').onclick = () => { store.set(ANN_KEY, String(a.at)); bar.remove(); };
    document.getElementById('top').after(bar);
  }).catch(() => {});
}

// ---------------------------------------------------------------- back to top

/** 「回到顶部」 above the 反馈 button, once a long page has scrolled two screens down. */
function topButton() {
  if (document.querySelector('.topfab')) return;
  const b = document.createElement('button');
  b.className = 'topfab';
  b.type = 'button';
  b.textContent = '↑';
  b.title = '回到顶部';
  b.setAttribute('aria-label', '回到顶部');
  b.hidden = true;
  b.onclick = () => window.scrollTo({ top: 0, behavior: matchMedia('(prefers-reduced-motion: reduce)').matches ? 'auto' : 'smooth' });
  document.body.appendChild(b);
  let raf = 0;
  const update = () => { raf = 0; b.hidden = window.scrollY < window.innerHeight * 2; };
  window.addEventListener('scroll', () => { if (!raf) raf = requestAnimationFrame(update); }, { passive: true });
}

// ---------------------------------------------------------------- feedback button

/** A floating 「反馈」 button on every page; the form goes to the admin 反馈 tab. */
function feedbackButton(show) {
  const old = document.querySelector('.fbfab');
  if (old) { old.hidden = !show; return; }
  if (!show) return;
  const b = document.createElement('button');
  b.className = 'fbfab';
  b.type = 'button';
  b.textContent = '反馈';
  b.title = '意见反馈';
  b.onclick = () => {
    const body = modal('意见反馈');
    const DK = 'xmuhub.feedback.draft';
    let d = {};
    try { d = JSON.parse(store.get(DK) || '{}') || {}; } catch { /* ignore */ }
    body.innerHTML = `<form id="fbq">
      <p class="small muted" style="margin-top:0">遇到问题、想要新功能、资料分类不对，都可以说。${COMMUNITY ? `也可以加${esc(COMMUNITY)}。` : ''}</p>
      <label class="field"><span>反馈内容 <em>*</em></span><textarea class="input" id="fbq_b" rows="5" maxlength="2000" required placeholder="尽量写清楚：在哪个页面、做了什么、看到了什么。"></textarea></label>
      <label class="field"><span>联系方式（选填：邮箱 / QQ）</span><input class="input" id="fbq_c" maxlength="100"></label>
      <p class="small faint">会自动附上当前页面地址：${esc(location.pathname + location.search)}</p>
      <button class="btn primary">提交</button></form>`;
    const tb = body.querySelector('#fbq_b');
    const tc = body.querySelector('#fbq_c');
    tb.value = d.body || '';
    tc.value = d.contact || '';
    const save = () => store.set(DK, tb.value.trim() || tc.value.trim() ? JSON.stringify({ body: tb.value, contact: tc.value }) : null);
    tb.oninput = save;
    tc.oninput = save;
    tb.focus();
    body.querySelector('#fbq').onsubmit = async (e) => {
      e.preventDefault();
      const btn = body.querySelector('#fbq button');
      btn.disabled = true;
      try {
        await api('/feedback', { method: 'POST', body: { body: tb.value, contact: tc.value, page: location.pathname + location.search } });
        store.set(DK, null);
        body.innerHTML = '<div class="notice ok">谢谢！我们已经收到你的反馈。</div>';
        setTimeout(() => body.close(), 1600);
      } catch (err) { toast(err.message, true); btn.disabled = false; }
    };
  };
  document.body.appendChild(b);
}

// ---------------------------------------------------------------- quick preview from lists

/**
 * 「预览」 on any resource list opens the file in a dialog straight away; ← / → (or the
 * buttons) step through the other files of the same list, so comparing a handful of
 * candidates takes a click each instead of a page visit each.
 */
function quickPreview(btn) {
  const scope = btn.closest('.list') || document;
  const all = [...scope.querySelectorAll('[data-qpv]')];
  let i = all.indexOf(btn);
  const body = modal('', { wide: true });
  const head = body.parentElement.querySelector('.modal-head');
  head.querySelector('b').insertAdjacentHTML('afterend', '<span class="qpv-nav"><button class="btn sm" data-nav="-1" type="button" aria-label="上一份">‹ 上一份</button><span class="small faint" data-pos></span><button class="btn sm" data-nav="1" type="button" aria-label="下一份">下一份 ›</button><a class="btn sm primary" data-open>详情 / 下载</a></span>');
  const show = () => {
    const b = all[i];
    const item = b.closest('.item');
    const title = item?.querySelector('.title')?.textContent || '';
    const sub = item?.querySelector('.subtitle')?.textContent || '';
    head.querySelector('b').textContent = title;
    head.querySelector('[data-pos]').textContent = all.length > 1 ? `${i + 1} / ${all.length}` : '';
    head.querySelectorAll('[data-nav]').forEach((n) => { n.hidden = all.length < 2; });
    head.querySelector('[data-open]').href = `/r/${b.dataset.qpv}`;
    body.innerHTML = `${sub ? `<p class="small muted" style="margin:0 0 8px">${esc(sub)}</p>` : ''}<div class="qpv-review"></div><div></div>`;
    const id = Number(b.dataset.qpv);
    preview(id, body.lastElementChild);
    reviewBar(id, body.querySelector('.qpv-review'));
  };
  // Reviewers decide right in the dialog; the next file opens by itself.
  const reviewBar = async (id, bar) => {
    const u = await me();
    if (!u || u.level < 3) return;
    let r;
    try { r = await api(`/resources/${id}`); } catch { return; }
    if (!bar.isConnected) return;
    const acts = [
      (r.status !== 'published' || r.needs_review || r.uncertain) && ['approve', r.status === 'published' ? '确认无误' : '通过', 'ok'],
      (r.status === 'pending' || r.status === 'restricted' || r.needs_review || r.uncertain) && ['reject', '驳回', 'danger'],
      r.status === 'published' && ['remove', '下架', 'danger'],
      (r.status === 'removed' || r.status === 'rejected' || r.status === 'restricted') && ['restore', '恢复发布', ''],
    ].filter(Boolean);
    if (!acts.length) return;
    bar.innerHTML = `<div class="row" style="margin:0 0 10px;gap:6px"><span class="small muted">审核：${esc(STATUS[r.status] || r.status)}${r.needs_review ? ' · 待复核' : ''}${r.uncertain ? ' · 待核实' : ''}</span>
      <input class="input" data-note placeholder="备注（驳回 / 下架原因）" style="max-width:220px;min-height:30px;padding:3px 8px">
      ${acts.map(([a, l, c]) => `<button class="btn sm ${c}" data-act="${a}" type="button">${l}</button>`).join('')}</div>`;
    bar.onclick = async (e) => {
      const btn = e.target.closest('[data-act]');
      if (!btn) return;
      btn.disabled = true;
      try {
        const res = await api(`/resources/${id}/review`, { method: 'POST', body: { action: btn.dataset.act, note: bar.querySelector('[data-note]').value } });
        toast(`已处理：${STATUS[res.status] || res.status}`);
        // Take it out of the list behind the dialog and move on.
        const item = all[i].closest('[data-id]') || all[i].closest('.item');
        all.splice(i, 1);
        item?.remove();
        if (!all.length) { body.close(); return; }
        if (i >= all.length) i = 0;
        show();
      } catch (err) { toast(err.message, true); btn.disabled = false; }
    };
  };
  const go = (d) => { i = (i + d + all.length) % all.length; show(); };
  head.onclick = (e) => { const n = e.target.closest('[data-nav]'); if (n) go(Number(n.dataset.nav)); };
  const onKey = (e) => {
    if (!body.isConnected) { document.removeEventListener('keydown', onKey); return; }
    if (e.target.closest('input, textarea')) return;
    if (e.key === 'ArrowLeft') go(-1);
    if (e.key === 'ArrowRight') go(1);
  };
  document.addEventListener('keydown', onKey);
  show();
}

// A thumbnail that fails on one mirror tries the next, then falls back to the type badge.
document.addEventListener('error', (e) => {
  const img = e.target;
  if (!(img instanceof HTMLImageElement) || !(img.classList.contains('thumb') || img.classList.contains('avatar'))) return;
  const alts = (img.dataset.alts || '').split(' ').filter(Boolean);
  if (alts.length) {
    img.dataset.alts = alts.slice(1).join(' ');
    img.src = alts[0];
  } else if (img.classList.contains('avatar')) {
    img.outerHTML = letterAvatar(img.dataset.name, parseInt(img.style.getPropertyValue('--s'), 10) || 28);
  } else {
    const d = document.createElement('div');
    d.className = `ficon ${img.dataset.ext || ''}`;
    d.textContent = img.dataset.ext || 'file';
    img.replaceWith(d);
  }
}, true);

document.addEventListener('click', (e) => {
  const thumb = e.target.closest('img.thumb');
  const b = thumb ? thumb.closest('.item')?.querySelector('[data-qpv]') : e.target.closest('[data-qpv]');
  if (!b) return;
  e.preventDefault();
  quickPreview(b);
});

// ---------------------------------------------------------------- page switches without reloading

/*
 * Links between the reading pages (home, categories, courses, files, search, help) don't
 * reload the page: the new page's <main> is fetched once per page type, swapped in and its
 * script run, while the header, background and loaded data stay. The address is a normal
 * URL, so refresh, bookmarks and back/forward work as before, and a refresh returns to the
 * same scroll position. Upload, account and admin pages still open with a full load.
 */
const SOFT = /^\/(?:|browse|help|about|feedback|links|missing|search|[nr]\/\d+\/?)$/;
const here = () => location.pathname + location.search;
const soft = (u) => u.origin === location.origin && SOFT.test(u.pathname) && SOFT.test(location.pathname);
const templates = new Map();
// Version of this app module (the server stamps it into every page, see versioning.rs).
const BUILD = document.querySelector('meta[name="xmuhub-version"]')?.content || '';
let navSeq = 0;
let shown = here();
let prevPage = '';

/** The page (path + query) the user came from, for feedback reports. */
export function referrer() {
  return prevPage || document.referrer.replace(location.origin, '');
}

/**
 * 「‹ 返回」 on course and file pages goes back to wherever the reader came from on this site,
 * at the same scroll position (a search, the home page, a long list); opened straight from a
 * shared link it goes up a level (its href) instead.
 */
document.addEventListener('click', (e) => {
  const a = e.target.closest('a.uplevel');
  if (!a || e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return;
  const from = referrer();
  if (!from.startsWith('/') || from.startsWith('/login') || history.length < 2) return;
  e.preventDefault();
  e.stopPropagation();
  history.back();
}, true);

/** <main>, title and script of a page type; /n/1 and /n/2 share one. Refetched after a while. */
function template(path) {
  const k = path.replace(/^\/([nr])\/\d+\/?$/, '/$1/');
  const hit = templates.get(k);
  if (hit && Date.now() - hit.at < STALE) return hit.p;
  const p = fetch(path, { credentials: 'same-origin' }).then(async (res) => {
    if (!res.ok || !(res.headers.get('content-type') || '').includes('text/html')) throw new Error(`HTTP ${res.status}`);
    const doc = new DOMParser().parseFromString(await res.text(), 'text/html');
    const main = doc.querySelector('main');
    const script = doc.querySelector('script[type="module"][src]');
    if (!main || !script) throw new Error('not a page');
    return {
      title: doc.title, desc: doc.querySelector('meta[name="description"]')?.content || '', page: doc.body.dataset.page || '',
      main: main.innerHTML, script: script.getAttribute('src'), build: doc.querySelector('meta[name="xmuhub-version"]')?.content || '',
    };
  });
  p.catch(() => templates.delete(k));
  templates.set(k, { p, at: Date.now() });
  return p;
}

/** Loads a module without running it, so running it later needs no network. */
function preloadModule(src) {
  return new Promise((resolve) => {
    const l = document.createElement('link');
    l.rel = 'modulepreload';
    l.href = src;
    l.onload = l.onerror = () => { l.remove(); resolve(); };
    document.head.append(l);
  });
}

function saveScroll() {
  try { history.replaceState({ ...history.state, y: Math.round(scrollY) }, ''); } catch { /* rate-limited */ }
}

/** Opens `url`: in place when both pages allow it, else as a normal page load. */
export function go(url, { replace = false } = {}) {
  const u = new URL(url, location.href);
  if (!soft(u)) { replace ? location.replace(u) : location.assign(u); return; }
  saveScroll();
  if (!replace) prevPage = here();
  history[replace ? 'replaceState' : 'pushState']({ y: 0 }, '', u.pathname + u.search + u.hash);
  render({ hash: u.hash });
}

async function render({ y = 0, hash = '' } = {}) {
  const seq = ++navSeq;
  shown = here();
  const root = document.documentElement;
  const busy = setTimeout(() => root.classList.add('nav-busy'), 150);
  let t;
  try {
    t = await template(location.pathname);
    // A newer deploy: load the page for real rather than run its scripts next to old ones.
    if (t.build !== BUILD) { location.reload(); return; }
    // A fresh query makes import() run the page module again; the script is already versioned.
    t.src = `${t.script}${t.script.includes('?') ? '&' : '?'}r=${seq}`;
    await preloadModule(t.src);
  } catch {
    location.reload();
    return;
  } finally {
    clearTimeout(busy);
    if (seq === navSeq) root.classList.remove('nav-busy');
  }
  if (seq !== navSeq) return;

  const vt = !!document.startViewTransition && !matchMedia('(prefers-reduced-motion: reduce)').matches;
  const swap = async () => {
    document.querySelectorAll('.modal .modal-body').forEach((b) => b.close?.());
    document.title = t.title;
    document.querySelector('meta[name="description"]')?.setAttribute('content', t.desc);
    document.body.dataset.page = t.page;
    const main = document.querySelector('main');
    main.classList.remove('nav-in');
    if (!vt) main.classList.add('nav-wait');
    main.innerHTML = t.main;
    root.classList.add('seen');
    window.scrollTo({ top: 0, behavior: 'instant' });
    try { await import(t.src); } catch { location.reload(); return; }
    // Wait (briefly) for the page's data, so it appears filled in instead of filling in.
    await settled(700);
    main.classList.remove('nav-wait');
    if (!vt) main.classList.add('nav-in');
    if (seq !== navSeq) return;
    const target = hash && document.getElementById(decodeURIComponent(hash.slice(1)));
    if (target) target.scrollIntoView({ behavior: 'instant' });
    else window.scrollTo({ top: y, behavior: 'instant' });
  };
  if (vt) {
    await document.startViewTransition(swap).updateCallbackDone.catch(() => {});
  } else {
    await swap();
  }
}

if ('scrollRestoration' in history && SOFT.test(location.pathname)) {
  history.scrollRestoration = 'manual';
  if (history.state?.y) {
    // A refresh or a return from another site: back to where the user was, once the data is in.
    const y = history.state.y;
    settled(1500).then(() => { if (navSeq === 0) window.scrollTo({ top: y, behavior: 'instant' }); });
  } else {
    try { history.replaceState({ ...history.state, y: 0 }, ''); } catch { /* ignore */ }
  }
  let timer;
  addEventListener('scroll', () => { clearTimeout(timer); timer = setTimeout(saveScroll, 250); }, { passive: true });
  addEventListener('pagehide', saveScroll);

  addEventListener('popstate', (e) => {
    if (!SOFT.test(location.pathname)) { location.reload(); return; }
    if (here() === shown) { // only the #anchor changed
      const t = location.hash && document.getElementById(decodeURIComponent(location.hash.slice(1)));
      if (t) t.scrollIntoView({ behavior: 'instant' }); else window.scrollTo({ top: e.state?.y || 0, behavior: 'instant' });
      return;
    }
    render({ y: e.state?.y || 0, hash: e.state?.y ? '' : location.hash });
  });

  document.addEventListener('click', (e) => {
    if (e.defaultPrevented || e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return;
    const a = e.target.closest('a[href]');
    if (!a || (a.target && a.target !== '_self') || a.hasAttribute('download') || typeof a.href !== 'string') return;
    const u = new URL(a.href);
    if (!soft(u)) return;
    if (u.pathname + u.search === here() && u.hash) return; // in-page anchor: the browser scrolls
    e.preventDefault();
    go(u, { replace: u.href === location.href });
  });

  document.addEventListener('submit', (e) => {
    const f = e.target;
    if (e.defaultPrevented || f.method.toLowerCase() !== 'get' || (f.target && f.target !== '_self')) return;
    const u = new URL(f.action);
    if (!soft(u)) return;
    e.preventDefault();
    u.search = new URLSearchParams(new FormData(f)).toString();
    go(u);
  });

  // Fetch a page type's template while the pointer is on its link.
  const warm = (e) => {
    const a = e.target.closest?.('a[href]');
    if (!a || typeof a.href !== 'string') return;
    const u = new URL(a.href);
    if (soft(u)) template(u.pathname).catch(() => {});
  };
  document.addEventListener('pointerover', warm, { passive: true });
  document.addEventListener('touchstart', warm, { passive: true });
}
