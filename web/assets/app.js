// XMUHub shared front-end helpers: API client, session, layout, formatting, downloads.
// Plain ES modules, no build step — edit and redeploy.

export const store = {
  get(k) { try { return localStorage.getItem(k); } catch { return null; } },
  set(k, v) { try { v == null ? localStorage.removeItem(k) : localStorage.setItem(k, v); } catch { /* private mode */ } },
};

export class ApiError extends Error {
  constructor(status, message) { super(message); this.status = status; }
}

/** JSON API call. Every non-GET carries the anti-CSRF header the server requires. */
export async function api(path, { method = 'GET', body, signal } = {}) {
  const headers = {};
  if (method !== 'GET') headers['X-XMUHub'] = '1';
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
  if (!res.ok) throw new ApiError(res.status, (data && data.error) || `请求失败（HTTP ${res.status}）`);
  return data;
}

let meCache = null;
/** The signed-in user or null. */
export function me() {
  if (!meCache) meCache = api('/me').then((r) => r.user).catch(() => null);
  return meCache;
}
export function forgetMe() { meCache = null; }

let metaCache = null;
export function meta() {
  if (!metaCache) metaCache = api('/meta');
  return metaCache;
}

let treeCache = null;
/** The whole category tree: { list, byId, children(id) }. */
export function tree() {
  if (!treeCache) {
    treeCache = api('/tree').then((list) => {
      const byId = new Map(list.map((n) => [n.id, n]));
      const kids = new Map();
      for (const n of list) {
        const k = n.parent ?? 0;
        if (!kids.has(k)) kids.set(k, []);
        kids.get(k).push(n);
      }
      return { list, byId, children: (id) => kids.get(id ?? 0) || [] };
    });
  }
  return treeCache;
}

/** Site slogan, shown big on the home page. */
export const SLOGAN = '让每一份资料都被需要它的人找到';
/** User group shown on the feedback page, e.g. 'QQ 群 123456789'; empty hides it. */
export const COMMUNITY = 'QQ 群：902541466';

export const LEVELS = ['访客', '贡献者', '可信贡献者', '审核员', '管理员'];

// ---------------------------------------------------------------- formatting

export function esc(s) {
  return String(s ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
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
    statusBadge(r),
  ].filter(Boolean);
  return `<div class="item">
    <div class="ficon ${esc(e)}">${esc(e.slice(0, 4))}</div>
    <div class="body"><a class="title" href="/r/${r.id}">${esc(r.title)}</a>
      ${r.note ? `<div class="note">${esc(r.note)}</div>` : ''}
      <div class="meta">${bits.join('')}</div></div>
  </div>`;
}

export function nodeCard(n, path) {
  const sub = path ? pathText(path) : (n.code || '');
  return `<a class="card node-card" href="/n/${n.id}">
    <h3>${esc(nodeTitle(n))}</h3>
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

export const qs = new URLSearchParams(location.search);

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
    <a class="brand" href="/"><img class="logo" src="/assets/logo.svg" alt=""><span><b>XMU</b>Hub<small>厦大资料库</small></span></a>
    ${active === 'home' || active === 'search' ? '<span class="grow"></span>' : `<form action="/search" role="search"><input name="q" value="${esc(q)}" placeholder="搜索课程、资料…" aria-label="搜索"></form>`}
    <nav class="nav">
      <a href="/browse" data-k="browse">分类</a>
      <a href="/admin" data-k="admin" hidden>审核</a>
      <a href="/me" data-k="me" id="nav-me">登录</a>
      <a href="/upload" data-k="upload" class="up">上传</a>
    </nav></div>`;
  top.querySelectorAll('.nav a').forEach((a) => { if (a.dataset.k === active && a.dataset.k !== 'upload') a.classList.add('on'); });
  const foot = document.getElementById('foot');
  foot.className = 'foot';
  foot.innerHTML = `<div class="wrap">
    <span>XMUHub · 厦门大学学生资料共享 · 非官方学生项目，与厦门大学官方无关</span>
    <span><a href="/about">使用须知</a> · <a href="/feedback">意见反馈</a>${COMMUNITY ? `（${esc(COMMUNITY)}）` : ''} · <a href="https://github.com/vintcessun/XMUHub" rel="noopener">源代码（AGPL-3.0）</a> · 资料由同学上传，仅供学习交流</span></div>`;
  const user = await me();
  const navMe = top.querySelector('#nav-me');
  if (user) {
    navMe.textContent = user.nickname;
    if (user.level >= 3) top.querySelector('[data-k="admin"]').hidden = false;
  } else {
    navMe.href = loginUrl();
  }
  return user;
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

/**
 * Fetches one part, trying each URL in turn. A mirror that is reachable but crawling
 * (under ~150 KB/s after a few seconds) is abandoned for the next one.
 */
async function fetchPart(part, urls, onBytes) {
  let lastErr;
  for (const [i, url] of urls.entries()) {
    const hasNext = i < urls.length - 1;
    const ctl = new AbortController();
    try {
      const startTimer = setTimeout(() => ctl.abort(), 15_000);
      const res = await fetch(url, { signal: ctl.signal, mode: 'cors', credentials: 'omit', referrerPolicy: 'no-referrer' });
      clearTimeout(startTimer);
      if (!res.ok || !res.body) throw new Error(`HTTP ${res.status}`);
      const reader = res.body.getReader();
      const chunks = [];
      const t0 = performance.now();
      let got = 0;
      for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        chunks.push(value);
        got += value.length;
        onBytes(got);
        const secs = (performance.now() - t0) / 1000;
        if (hasNext && secs > 6 && got / secs < 150 * 1024) { ctl.abort(); throw new Error('镜像太慢'); }
      }
      if (got !== part.size) throw new Error('大小不符');
      return new Blob(chunks);
    } catch (e) {
      lastErr = e;
      onBytes(0);
    }
  }
  throw lastErr || new Error('没有可用的下载地址');
}

/**
 * Downloads a resource. Single files: try a cross-origin fetch through the fastest mirror
 * (keeps the Chinese filename); most mirrors send no CORS headers, in which case we navigate
 * to that same fastest mirror (full speed, ASCII filename). Multi-part files must be fetched
 * and joined in the browser, so every mirror is tried for each part.
 */
export async function downloadResource(id, onProgress = () => {}) {
  const plan = await api(`/resources/${id}/download`);
  const total = plan.size;
  const single = plan.parts.length === 1;
  let doneBytes = 0;
  try {
    const blobs = [];
    for (const part of plan.parts) {
      const urls = single ? part.urls.slice(0, 1) : part.urls;
      const b = await fetchPart(part, urls, (n) => onProgress(Math.min(1, (doneBytes + n) / total)));
      doneBytes += part.size;
      blobs.push(b);
    }
    saveBlob(new Blob(blobs, { type: plan.mime || 'application/octet-stream' }), plan.filename);
    return { ok: true };
  } catch (e) {
    if (single) {
      location.href = plan.parts[0].urls[0];
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

const INLINE = { pdf: 'application/pdf', png: 'image/png', jpg: 'image/jpeg', jpeg: 'image/jpeg', gif: 'image/gif', webp: 'image/webp', txt: 'text/plain;charset=utf-8', md: 'text/plain;charset=utf-8' };
const OFFICE = { doc: 10, docx: 10, ppt: 10, pptx: 10, pptm: 10, xls: 5, xlsx: 5 };
const PREVIEW_MAX = 60 * 1024 * 1024;

/**
 * Renders a preview of a resource into `box`. PDFs, images and text are fetched in the
 * browser through a mirror that allows cross-origin reads (never through our server);
 * Office files use Microsoft's online viewer. Previews don't count as downloads.
 */
export async function preview(id, box) {
  box.innerHTML = '<p class="small muted">正在加载预览…</p>';
  let plan;
  try { plan = await api(`/resources/${id}/download?peek=1`); } catch (e) { box.innerHTML = `<div class="notice bad">${esc(e.message)}</div>`; return; }
  const ext = (/\.([^.]+)$/.exec(plan.filename)?.[1] || '').toLowerCase();
  const part = plan.parts[0];
  const fallback = (msg) => { box.innerHTML = `<div class="notice">${msg}</div>`; };
  if (plan.parts.length === 1 && INLINE[ext]) {
    if (plan.size > PREVIEW_MAX) return fallback(`文件较大（${fmtSize(plan.size)}），请下载后查看。`);
    const urls = part.preview_urls || [];
    if (!urls.length) return fallback('暂时没有支持在线预览的镜像，请下载后查看。');
    box.innerHTML = '<p class="small muted">正在通过镜像加载预览…</p><div class="progress"><i></i></div>';
    const bar = box.querySelector('.progress i');
    let blob;
    try {
      blob = await fetchPart(part, urls, (n) => { bar.style.width = `${Math.round((n / part.size) * 100)}%`; });
    } catch (e) {
      return fallback(`预览加载失败（${esc(e.message)}），请下载后查看。`);
    }
    if (!box.isConnected) return;
    const url = URL.createObjectURL(new Blob([blob], { type: INLINE[ext] }));
    const open = `<p class="small" style="margin:8px 0 0"><a href="${url}" target="_blank" rel="noopener">在新标签页打开</a> · 手机上显示空白时点这里或直接下载</p>`;
    box.innerHTML = (INLINE[ext].startsWith('image/')
      ? `<img class="preview-img" src="${url}" alt="预览">`
      : `<iframe class="preview-frame" src="${url}" title="预览"></iframe>`) + open;
    return;
  }
  if (plan.parts.length === 1 && OFFICE[ext]) {
    if (plan.size > OFFICE[ext] * 1024 * 1024) return fallback(`文件较大（${fmtSize(plan.size)}），在线预览最大 ${OFFICE[ext]} MB，请下载后查看。`);
    // Microsoft fetches the file itself, so give it the plain GitHub URL (the last one).
    const direct = part.urls[part.urls.length - 1];
    const src = `https://view.officeapps.live.com/op/embed.aspx?src=${encodeURIComponent(direct)}`;
    box.innerHTML = `<iframe class="preview-frame" src="${esc(src)}" title="预览"></iframe>
      <p class="small faint" style="margin:8px 0 0">由微软 Office 在线查看器显示，加载可能需要十几秒。</p>`;
    return;
  }
  fallback(plan.parts.length > 1 ? '分卷文件不支持在线预览，请下载后查看。' : `.${esc(ext || '未知')} 文件不支持在线预览，请下载后查看。`);
}
