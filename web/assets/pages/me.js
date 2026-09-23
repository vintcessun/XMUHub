import { api, esc, LEVELS, layout, resourceItem, token, toast, $ } from '../app.js';

const mePromise = layout('me');

function pasteForm() {
  return `<form id="paste" class="row" style="margin-top:12px">
    <input class="input grow" id="secret" placeholder="粘贴令牌（以 xh 开头）" autocomplete="off" style="min-width:220px">
    <button class="btn">使用这个令牌</button></form>`;
}

async function render() {
  const me = await mePromise;
  const card = $('#tokencard');
  const secret = token.get();
  if (me.level > 0 && secret) {
    card.innerHTML = `<h2>当前身份：${LEVELS[me.level]}</h2>
      <p class="muted small">${me.label ? esc(me.label) + ' · ' : ''}令牌编号 #${me.id} · 已上传 ${me.uploads} 份</p>
      <p class="small">这是你的令牌，<b>请复制保存</b>。换浏览器或清除缓存后，把它粘贴回来就能继续管理你的上传：</p>
      <div class="secret" id="sec">${'•'.repeat(24)}</div>
      <div class="row" style="margin-top:10px"><button class="btn sm" id="show">显示</button><button class="btn sm" id="copy">复制</button><span class="grow"></span><button class="btn sm danger" id="forget">从本浏览器移除</button></div>
      <details style="margin-top:14px"><summary class="small">切换到另一个令牌</summary>${pasteForm()}</details>`;
    $('#show').onclick = () => { $('#sec').textContent = secret; };
    $('#copy').onclick = async () => {
      try { await navigator.clipboard.writeText(secret); toast('已复制'); } catch { $('#sec').textContent = secret; toast('请手动选择复制', true); }
    };
    $('#forget').onclick = () => {
      if (!confirm('移除后需要重新粘贴令牌才能管理你的上传。确定吗？')) return;
      token.set(null);
      location.reload();
    };
    loadMine();
  } else {
    if (secret) token.set(null); // revoked or invalid
    card.innerHTML = `<h2>当前身份：访客</h2>
      <p class="muted">浏览和下载不需要令牌。想上传资料的话，领取一个贡献者令牌即可。</p>
      <div class="row"><button class="btn primary" id="claim">领取贡献者令牌</button></div>
      ${pasteForm()}`;
    $('#claim').onclick = async () => {
      try {
        const r = await api('/token/claim', { method: 'POST' });
        token.set(r.token);
        location.reload();
      } catch (e) { toast(e.message, true); }
    };
  }
  $('#paste').onsubmit = async (e) => {
    e.preventDefault();
    const s = $('#secret').value.trim();
    if (!s) return;
    const old = token.get();
    token.set(s);
    const m = await api('/me').catch(() => ({ level: 0 }));
    if (m.level > 0) {
      toast(`已切换为 ${LEVELS[m.level]}`);
      location.reload();
    } else {
      token.set(old);
      toast('令牌无效或已被停用', true);
    }
  };
}

async function loadMine() {
  $('#minecard').hidden = false;
  try {
    const rs = await api('/mine');
    $('#mine').innerHTML = rs.length ? rs.map((r) => resourceItem(r)).join('') : '<div class="empty"><b>还没有上传过资料</b><a href="/upload">去上传</a></div>';
  } catch (e) {
    $('#mine').innerHTML = `<div class="notice bad">${esc(e.message)}</div>`;
  }
}

render();
