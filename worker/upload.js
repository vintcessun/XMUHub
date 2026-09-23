// XMUHub upload relay (Cloudflare Worker).
//
// The browser POSTs a file part here with a ticket signed by the XMUHub server; the Worker
// checks the ticket and streams the bytes straight into a GitHub release asset using the
// storage account's token (kept in a Worker secret, never sent to browsers). The XMUHub
// server never touches the bytes and later verifies the asset via the GitHub API.
//
// Bindings: GH_TOKEN (secret), TICKET_SECRET (secret), GH_OWNER, ALLOWED_ORIGINS (comma list).

const enc = new TextEncoder();

function b64urlToBytes(s) {
  s = s.replace(/-/g, '+').replace(/_/g, '/');
  while (s.length % 4) s += '=';
  const bin = atob(s);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

// Ticket format (see crates/xmuhub-core/src/ticket.rs): base64url(json) "." base64url(hmac_sha256)
async function verifyTicket(ticket, secret) {
  if (!ticket || !secret) return null;
  const dot = ticket.indexOf('.');
  if (dot < 0) return null;
  const payload = ticket.slice(0, dot);
  let sig;
  try { sig = b64urlToBytes(ticket.slice(dot + 1)); } catch { return null; }
  const key = await crypto.subtle.importKey('raw', enc.encode(secret), { name: 'HMAC', hash: 'SHA-256' }, false, ['verify']);
  if (!(await crypto.subtle.verify('HMAC', key, sig, enc.encode(payload)))) return null;
  let t;
  try { t = JSON.parse(new TextDecoder().decode(b64urlToBytes(payload))); } catch { return null; }
  if (typeof t.u !== 'string' || typeof t.s !== 'number' || typeof t.e !== 'number') return null;
  if (t.e < Date.now() / 1000) return null;
  return t;
}

function corsHeaders(req, env) {
  const allowed = (env.ALLOWED_ORIGINS || '').split(',').map((s) => s.trim()).filter(Boolean);
  const origin = req.headers.get('Origin') || '';
  return {
    'Access-Control-Allow-Origin': allowed.includes(origin) ? origin : (allowed[0] || '*'),
    'Access-Control-Allow-Methods': 'POST, OPTIONS',
    'Access-Control-Allow-Headers': 'Content-Type',
    'Access-Control-Max-Age': '86400',
    Vary: 'Origin',
  };
}

function json(body, status, headers) {
  return new Response(JSON.stringify(body), { status, headers: { ...headers, 'Content-Type': 'application/json' } });
}

export default {
  async fetch(req, env) {
    const cors = corsHeaders(req, env);
    const url = new URL(req.url);
    if (req.method === 'OPTIONS') return new Response(null, { status: 204, headers: cors });
    if (url.pathname === '/health') return json({ ok: true }, 200, cors);
    if (url.pathname !== '/upload' || req.method !== 'POST') return json({ error: 'not found' }, 404, cors);

    const t = await verifyTicket(url.searchParams.get('t'), env.TICKET_SECRET);
    if (!t) return json({ error: '上传凭证无效或已过期，请刷新页面重试' }, 403, cors);
    // Even a leaked secret can only write into the storage account's release assets.
    if (!t.u.startsWith(`https://uploads.github.com/repos/${env.GH_OWNER}/`)) {
      return json({ error: 'bad destination' }, 403, cors);
    }
    const len = Number(req.headers.get('Content-Length'));
    if (!req.body || len !== t.s) return json({ error: `文件大小不符（${len} ≠ ${t.s}）` }, 400, cors);

    // GitHub rejects chunked uploads; FixedLengthStream makes the outgoing request carry
    // Content-Length while still streaming (no buffering of the whole part in the Worker).
    const { readable, writable } = new FixedLengthStream(t.s);
    const pump = req.body.pipeTo(writable).catch(() => {});
    let gh;
    try {
      gh = await fetch(t.u, {
        method: 'POST',
        headers: {
          Authorization: `Bearer ${env.GH_TOKEN}`,
          Accept: 'application/vnd.github+json',
          'X-GitHub-Api-Version': '2022-11-28',
          'Content-Type': 'application/octet-stream',
          'User-Agent': 'XMUHub-upload-worker',
        },
        body: readable,
      });
    } catch (e) {
      return json({ error: `转存到 GitHub 失败：${e.message}` }, 502, cors);
    }
    await pump;
    const text = await gh.text();
    if (!gh.ok) {
      let msg = text.slice(0, 300);
      try { msg = JSON.parse(text).message || msg; } catch { /* keep raw */ }
      return json({ error: `GitHub 返回 ${gh.status}：${msg}` }, gh.status === 422 ? 409 : 502, cors);
    }
    // Pass GitHub's asset JSON through; the browser forwards `id` to XMUHub for verification.
    return new Response(text, { status: 201, headers: { ...cors, 'Content-Type': 'application/json' } });
  },
};
