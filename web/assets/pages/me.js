import { ago, api, esc, fmtDate, forgetMe, LEVELS, layout, loginUrl, pwToggle, resourceItem, toast, $ } from '../app.js';

pwToggle($('#oldpw'), $('#newpw'), $('#newpw2'));

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
    if ($('#newpw').value !== $('#newpw2').value) return toast('两次输入的新密码不一致', true);
    try {
      await api('/me', { method: 'PATCH', body: { old_password: $('#oldpw').value, new_password: $('#newpw').value } });
      toast('密码已修改，其他设备已退出登录');
      $('#oldpw').value = $('#newpw').value = $('#newpw2').value = '';
    } catch (err) { toast(err.message, true); }
  };

  loadTokens();
  $('#tf').onsubmit = async (e) => {
    e.preventDefault();
    try {
      const r = await api('/me/tokens', { method: 'POST', body: { name: $('#tname').value } });
      $('#tname').value = '';
      $('#tnew').innerHTML = `<div class="notice ok" style="margin-top:12px">令牌已创建，<b>只显示这一次</b>，请立即复制保存：
        <div class="row" style="margin-top:8px;flex-wrap:nowrap"><input class="input mono" id="tsecret" readonly value="${esc(r.secret)}"><button class="btn sm" id="tcopy" type="button">复制</button></div></div>`;
      $('#tcopy').onclick = async () => {
        try { await navigator.clipboard.writeText(r.secret); toast('已复制'); } catch { $('#tsecret').select(); toast('请手动复制', true); }
      };
      loadTokens();
    } catch (err) { toast(err.message, true); }
  };
  $('#tlist').onclick = async (e) => {
    const b = e.target.closest('[data-del]');
    if (!b) return;
    try { await api(`/me/tokens/${b.dataset.del}`, { method: 'DELETE' }); toast('已删除，使用它的客户端将无法再连接'); loadTokens(); } catch (err) { toast(err.message, true); }
  };

  try {
    const rs = await api('/mine');
    if (!rs.length) {
      $('#mine').innerHTML = '<div class="empty"><b>还没有上传过资料</b><a href="/upload">去上传</a></div>';
      return;
    }
    const render = () => {
      const q = $('#mq').value.trim().toLowerCase();
      const st = $('#ms').value;
      const hit = rs.filter((r) => (!st || r.status === st) && (!q || [r.title, r.node.name, r.note].some((x) => (x || '').toLowerCase().includes(q))));
      $('#mcount').textContent = q || st ? `找到 ${hit.length} / ${rs.length} 份` : `共 ${rs.length} 份`;
      $('#mine').innerHTML = hit.length ? hit.map((r) => resourceItem(r)).join('') : '<div class="empty">没有符合条件的资料</div>';
    };
    $('#mq').oninput = render;
    $('#ms').onchange = render;
    render();
  } catch (e) {
    $('#mine').innerHTML = `<div class="notice bad">${esc(e.message)}</div>`;
  }
})();

async function loadTokens() {
  try {
    const list = await api('/me/tokens');
    $('#tlist').innerHTML = list.length ? `<div class="scroll-x"><table class="table"><thead><tr><th>用途</th><th>令牌</th><th>创建</th><th>最近使用</th><th></th></tr></thead><tbody>
      ${list.map((t) => `<tr><td>${esc(t.name)}</td><td class="mono small">${esc(t.prefix)}…</td><td class="small faint">${fmtDate(t.created_at)}</td>
        <td class="small faint">${t.last_used ? ago(t.last_used) : '未使用'}</td><td><button class="btn sm danger" data-del="${t.id}">删除</button></td></tr>`).join('')}
      </tbody></table></div>` : '<p class="small faint">还没有令牌</p>';
  } catch (e) { $('#tlist').innerHTML = `<div class="notice bad">${esc(e.message)}</div>`; }
}
