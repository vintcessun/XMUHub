import { ago, api, COMMUNITY, esc, layout, loginUrl, qs, referrer, store, toast, $ } from '../app.js';

// Feedback and takedown requests come from signed-in users only (replies reach the account).
const me = await layout('feedback');
if (!me) {
  $('#f').innerHTML = `<div class="notice">意见反馈和申请下架需要先登录，处理结果会通过站内提醒和你的账号邮箱告诉你。<div style="margin-top:10px"><a class="btn primary" href="${loginUrl()}">登录 / 注册</a></div></div>`;
}

if (COMMUNITY) {
  $('#group').hidden = false;
  $('#group').innerHTML = `<h3>用户群</h3><p style="margin:0">${esc(COMMUNITY)}</p>`;
}

if (me) {
  // Unsent feedback survives a refresh.
  const DK = 'xmuhub.feedback.draft';
  try { const d = JSON.parse(store.get(DK) || 'null'); if (d) { $('#body').value = d.body || ''; $('#contact').value = d.contact || ''; } } catch { /* ignore */ }
  const saveDraft = () => store.set(DK, $('#body').value.trim() || $('#contact').value.trim() ? JSON.stringify({ body: $('#body').value, contact: $('#contact').value }) : null);
  // From an empty search (「告诉我们缺这门课」): start the message with what was searched for.
  const want = (qs.get('want') || '').trim().slice(0, 60);
  if (want && !$('#body').value.trim()) $('#body').value = `想找「${want}」的资料，本站还没有。课程全称 / 老师 / 学院（选填）：`;
  $('#body').oninput = saveDraft;
  $('#contact').oninput = saveDraft;

  // 申请下架: rights holders must show the file is theirs; they are reached by email (the account's
  // unless they give another) for more proof, and a claim without proof is turned down.
  const TAKEDOWN = '【申请下架】';
  const EMAIL = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;
  const takedownMode = () => {
    const on = $('#takedown').checked;
    $('#takedown-help').hidden = !on;
    $('#contact').type = on ? 'email' : 'text';
    $('#contact').placeholder = on ? me.email : '';
    $('#contact-label').innerHTML = on ? `联系邮箱（核实和补充证明材料都通过邮箱联系你；不填就用账号邮箱 ${esc(me.email)}）` : '联系方式（选填，不填就用你的账号邮箱回复）';
    if (on && !$('#body').value.trim()) $('#body').value = `资料链接：${qs.get('from') ? location.origin + qs.get('from') : ''}\n我的身份 / 权利说明：\n证明材料（证明这份资料属于我）：\n`;
  };
  $('#takedown').checked = qs.get('takedown') === '1';
  $('#takedown').onchange = takedownMode;
  takedownMode();

  $('#f').onsubmit = async (e) => {
    e.preventDefault();
    const takedown = $('#takedown').checked;
    const contact = $('#contact').value.trim();
    if (takedown && contact && !EMAIL.test(contact)) { $('#msg').innerHTML = '<div class="notice bad">请填写正确的邮箱，或者留空使用账号邮箱。</div>'; return; }
    const btn = $('#f button');
    btn.disabled = true;
    try {
      const body = takedown && !$('#body').value.startsWith(TAKEDOWN) ? TAKEDOWN + $('#body').value : $('#body').value;
      const r = await api('/feedback', { method: 'POST', body: { body, contact, page: qs.get('from') || referrer() } });
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
}

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
