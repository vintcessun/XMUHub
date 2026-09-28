import { ago, api, COMMUNITY, esc, layout, qs, referrer, store, toast, $ } from '../app.js';

layout('feedback');

if (COMMUNITY) {
  $('#group').hidden = false;
  $('#group').innerHTML = `<h3>用户群</h3><p style="margin:0">${esc(COMMUNITY)}</p>`;
}

// Unsent feedback survives a refresh.
const DK = 'xmuhub.feedback.draft';
try { const d = JSON.parse(store.get(DK) || 'null'); if (d) { $('#body').value = d.body || ''; $('#contact').value = d.contact || ''; } } catch { /* ignore */ }
const saveDraft = () => store.set(DK, $('#body').value.trim() || $('#contact').value.trim() ? JSON.stringify({ body: $('#body').value, contact: $('#contact').value }) : null);
$('#body').oninput = saveDraft;
$('#contact').oninput = saveDraft;

$('#f').onsubmit = async (e) => {
  e.preventDefault();
  const btn = $('#f button');
  btn.disabled = true;
  try {
    const r = await api('/feedback', { method: 'POST', body: { body: $('#body').value, contact: $('#contact').value, page: qs.get('from') || referrer() } });
    store.set(DK, null);
    remember(r);
    $('#f').innerHTML = '<div class="notice ok">谢谢！我们已经收到你的反馈，处理结果会显示在下面的「我的反馈」里。</div>';
    toast('已提交');
    loadMine();
  } catch (err) {
    $('#msg').innerHTML = `<div class="notice bad">${esc(err.message)}</div>`;
    btn.disabled = false;
  }
};

// ---------------------------------------------------------------- my feedback and its replies
//
// Each submission returns a receipt (id + key) kept in this browser, so a sender without an
// account can still see the reply; signed-in senders also see everything from their account.

const MINE = 'xmuhub.feedback.mine';
function receipts() {
  try { return JSON.parse(store.get(MINE) || '[]'); } catch { return []; }
}
function remember(r) {
  if (!r || !r.id || !r.key) return;
  store.set(MINE, JSON.stringify([{ id: r.id, key: r.key }, ...receipts().filter((x) => x.id !== r.id)].slice(0, 30)));
}

async function loadMine() {
  const k = receipts().map((x) => `${x.id}.${x.key}`).join(',');
  let list = [];
  try { list = await api(`/feedback/mine?k=${encodeURIComponent(k)}`); } catch { return; }
  $('#mine').hidden = !list.length;
  $('#minelist').innerHTML = list.map((f) => `<div class="item"><div class="body">
      <div style="white-space:pre-wrap;overflow-wrap:anywhere">${esc(f.body)}</div>
      <div class="meta"><span>${ago(f.created_at)}</span>${f.handled ? '<span class="badge published">已处理</span>' : '<span class="badge pending">等待处理</span>'}</div>
      ${f.reply ? `<div class="notice ok" style="margin-top:8px"><b>管理员回复：</b>${esc(f.reply)}</div>` : ''}
    </div></div>`).join('');
}
loadMine();
// Replies show up without reloading while the page is open (and on coming back to it).
setInterval(() => { if (document.visibilityState === 'visible') loadMine(); }, 60_000);
document.addEventListener('visibilitychange', () => { if (document.visibilityState === 'visible') loadMine(); });
