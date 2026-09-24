import { api, forgetMe, layout, meta, pwToggle, qs, toast, $ } from '../app.js';

pwToggle($('#password'), $('#password2'));

layout('me').then((u) => { if (u) location.replace(next()); });

function next() {
  const n = qs.get('next') || '/me';
  // Only same-site paths, never an absolute URL (open-redirect guard).
  return n.startsWith('/') && !n.startsWith('//') ? n : '/me';
}

let mode = qs.get('mode') || 'login';

function setMode(m) {
  mode = m;
  document.querySelectorAll('#tabs button').forEach((b) => b.classList.toggle('on', b.dataset.t === m));
  $('#codebox').hidden = m === 'login';
  $('#nickbox').hidden = m !== 'register';
  $('#pw2box').hidden = m === 'login';
  $('#password2').required = m !== 'login';
  $('#pwlabel').textContent = m === 'login' ? '密码' : m === 'reset' ? '新密码（至少 8 位）' : '设置密码（至少 8 位）';
  $('#password').autocomplete = m === 'login' ? 'current-password' : 'new-password';
  $('#go').textContent = { login: '登录', register: '注册并登录', reset: '重置密码并登录' }[m];
  $('#msg').innerHTML = '';
}

$('#tabs').onclick = (e) => { const b = e.target.closest('button'); if (b) setMode(b.dataset.t); };

let cooldown = 0;
function tick() {
  const b = $('#sendcode');
  if (cooldown > 0) { b.disabled = true; b.textContent = `${cooldown} 秒`; cooldown--; setTimeout(tick, 1000); }
  else { b.disabled = false; b.textContent = '获取验证码'; }
}

$('#sendcode').onclick = async () => {
  const email = $('#email').value.trim();
  if (!email) return toast('请先填写邮箱', true);
  $('#sendcode').disabled = true;
  try {
    await api('/auth/code', { method: 'POST', body: { email, purpose: mode === 'reset' ? 'reset' : 'register' } });
    toast('验证码已发送，请查收邮件（也看看垃圾箱）');
    cooldown = 60;
    tick();
  } catch (e) {
    toast(e.message, true);
    $('#sendcode').disabled = false;
  }
};

$('#f').onsubmit = async (e) => {
  e.preventDefault();
  if (mode !== 'login' && $('#password').value !== $('#password2').value) {
    $('#msg').innerHTML = '<div class="notice bad">两次输入的密码不一致</div>';
    return;
  }
  const body = { email: $('#email').value.trim(), password: $('#password').value };
  if (mode !== 'login') body.code = $('#code').value.trim();
  if (mode === 'register') body.nickname = $('#nickname').value.trim();
  $('#go').disabled = true;
  try {
    await api(`/auth/${mode}`, { method: 'POST', body });
    forgetMe();
    location.replace(next());
  } catch (err) {
    $('#msg').innerHTML = `<div class="notice bad">${err.message.replace(/[<>&]/g, '')}</div>`;
  } finally {
    $('#go').disabled = false;
  }
};

meta().then((m) => {
  if (!m.mail) {
    for (const t of ['register', 'reset']) document.querySelector(`#tabs [data-t="${t}"]`).title = '邮件服务未开启';
  }
});
setMode(mode);
