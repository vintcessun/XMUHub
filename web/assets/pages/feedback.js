import { api, COMMUNITY, esc, layout, qs, referrer, store, toast, $ } from '../app.js';

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
    await api('/feedback', { method: 'POST', body: { body: $('#body').value, contact: $('#contact').value, page: qs.get('from') || referrer() } });
    store.set(DK, null);
    $('#f').innerHTML = '<div class="notice ok">谢谢！我们已经收到你的反馈。</div>';
    toast('已提交');
  } catch (err) {
    $('#msg').innerHTML = `<div class="notice bad">${esc(err.message)}</div>`;
    btn.disabled = false;
  }
};
