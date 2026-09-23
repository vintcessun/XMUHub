import { api, esc, forgetMe, LEVELS, layout, loginUrl, resourceItem, toast, $ } from '../app.js';

(async () => {
  const me = await layout('me');
  if (!me) { location.replace(loginUrl()); return; }
  $('#who').innerHTML = `${esc(me.nickname)} · ${esc(me.email)} · ${LEVELS[me.level]}${me.xmu ? ' · <span class="badge published">厦大认证</span>' : ''}`;
  $('#nick').value = me.nickname;

  $('#logout').onclick = async () => {
    try { await api('/auth/logout', { method: 'POST' }); } catch { /* already gone */ }
    forgetMe();
    location.href = '/';
  };
  $('#nickf').onsubmit = async (e) => {
    e.preventDefault();
    try { await api('/me', { method: 'PATCH', body: { nickname: $('#nick').value } }); toast('已保存'); } catch (err) { toast(err.message, true); }
  };
  $('#pwf').onsubmit = async (e) => {
    e.preventDefault();
    try {
      await api('/me', { method: 'PATCH', body: { old_password: $('#oldpw').value, new_password: $('#newpw').value } });
      toast('密码已修改，其他设备已退出登录');
      $('#oldpw').value = $('#newpw').value = '';
    } catch (err) { toast(err.message, true); }
  };

  try {
    const rs = await api('/mine');
    $('#mine').innerHTML = rs.length ? rs.map((r) => resourceItem(r)).join('') : '<div class="empty"><b>还没有上传过资料</b><a href="/upload">去上传</a></div>';
  } catch (e) {
    $('#mine').innerHTML = `<div class="notice bad">${esc(e.message)}</div>`;
  }
})();
