import { ago, api, avatar, esc, fmtDate, forgetMe, LEVELS, layout, loginUrl, pwToggle, resourceItem, sendPart, toast, $ } from '../app.js';

pwToggle($('#oldpw'), $('#newpw'), $('#newpw2'));

(async () => {
  const me = await layout('me');
  if (!me) { location.replace(loginUrl()); return; }
  $('#who').innerHTML = `${esc(me.nickname)} · ${esc(me.email)} · ${LEVELS[me.level]}${me.xmu ? ' · <span class="badge published">厦大认证</span>' : ''}`;
  $('#nick').value = me.nickname;
  // Reviewers asking about the caller's uploads (answered on each file's page).
  api('/me/questions').then((list) => {
    if (!list.length) return;
    $('#questions').hidden = false;
    $('#questions').innerHTML = `<div class="notice warn"><b>审核员有 ${list.length} 个问题等你回答</b>（回答后资料会继续审核）：
      ${list.map(({ question: q, title }) => `<div class="small" style="margin-top:6px">· <a href="/r/${q.resource}">${esc(title)}</a>：${esc(q.text)}</div>`).join('')}</div>`;
  }).catch(() => {});
  $('#pubname').checked = !!me.public_name;
  $('#pubname').onchange = async (e) => {
    try {
      await api('/me', { method: 'PATCH', body: { public_name: e.target.checked } });
      toast(e.target.checked ? '已公开：你上传的资料页会显示你的昵称' : '已设为不公开');
    } catch (err) { e.target.checked = !e.target.checked; toast(err.message, true); }
  };

  showAvatar(me);
  $('#avfile').onchange = async (e) => {
    const f = e.target.files[0];
    e.target.value = '';
    if (!f) return;
    $('#avnote').textContent = '正在上传…';
    try {
      const now = await uploadAvatar(f);
      showAvatar(now);
      Object.assign(me, now);
      const nav = document.querySelector('#nav-me');
      if (nav) nav.innerHTML = `${avatar(now.avatar, now.nickname, 22)}<span>${esc(now.nickname)}</span>`;
      forgetMe(); // the next page rebuilds the header with the new picture
      toast('已提交，审核通过后别人就能看到');
    } catch (err) { toast(err.message, true); showAvatar(me); }
  };
  $('#avclear').onclick = async () => {
    if (!confirm('移除头像？')) return;
    try {
      await api('/me/avatar', { method: 'DELETE' });
      me.avatar = []; me.avatar_pending = [];
      showAvatar(me);
      const nav = document.querySelector('#nav-me');
      if (nav) nav.innerHTML = `${avatar([], me.nickname, 22)}<span>${esc(me.nickname)}</span>`;
      forgetMe();
    } catch (err) { toast(err.message, true); }
  };

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

  loadMine();
})();

// ---------------------------------------------------------------- my uploads, a page at a time
//
// Some people have uploaded thousands of files; rendering them all at once froze phones. The
// server filters and pages the list; more is fetched as the end of the list scrolls into view.

const PAGE = 30;
let mineSeq = 0;
let mineShown = 0;
let mineMatched = 0;
let mineLoading = false;
const mineMore = new IntersectionObserver((entries) => {
  if (entries.some((e) => e.isIntersecting)) loadMine(true);
}, { rootMargin: '600px' });

async function loadMine(more = false) {
  if (more && (mineLoading || mineShown >= mineMatched)) return;
  const my = more ? mineSeq : ++mineSeq;
  mineLoading = true;
  const q = $('#mq').value.trim();
  const st = $('#ms').value;
  const params = new URLSearchParams({ offset: String(more ? mineShown : 0), limit: String(PAGE) });
  if (q) params.set('q', q);
  if (st) params.set('status', st);
  try {
    const r = await api(`/mine?${params}`);
    if (my !== mineSeq) return; // a newer search started meanwhile
    const list = $('#mine');
    document.getElementById('minemore')?.remove();
    if (!more) {
      mineShown = 0;
      list.innerHTML = '';
      if (!r.total) { list.innerHTML = '<div class="empty"><b>还没有上传过资料</b><a href="/upload">去上传</a></div>'; $('#mcount').textContent = ''; return; }
      if (!r.matched) { list.innerHTML = '<div class="empty">没有符合条件的资料</div>'; }
    }
    mineMatched = r.matched;
    mineShown += r.items.length;
    list.insertAdjacentHTML('beforeend', r.items.map((x) => resourceItem(x)).join(''));
    $('#mcount').textContent = q || st ? `找到 ${r.matched} / ${r.total} 份` : `共 ${r.total} 份`;
    if (mineShown < mineMatched) {
      list.insertAdjacentHTML('beforeend', `<div id="minemore" class="small muted" style="text-align:center;padding:12px">已显示 ${mineShown} / ${mineMatched}，继续往下滚动加载更多</div>`);
      mineMore.observe(document.getElementById('minemore'));
    }
  } catch (e) {
    if (!more) $('#mine').innerHTML = `<div class="notice bad">${esc(e.message)}</div>`;
    else toast(e.message, true);
  } finally {
    if (my === mineSeq) mineLoading = false;
  }
}

let mineTimer = 0;
$('#mq').oninput = () => { clearTimeout(mineTimer); mineTimer = setTimeout(() => loadMine(), 300); };
$('#ms').onchange = () => loadMine();

async function loadTokens() {
  try {
    const list = await api('/me/tokens');
    $('#tlist').innerHTML = list.length ? `<div class="scroll-x"><table class="table"><thead><tr><th>用途</th><th>令牌</th><th>创建</th><th>最近使用</th><th></th></tr></thead><tbody>
      ${list.map((t) => `<tr><td>${esc(t.name)}</td><td class="mono small">${esc(t.prefix)}…</td><td class="small faint">${fmtDate(t.created_at)}</td>
        <td class="small faint">${t.last_used ? ago(t.last_used) : '未使用'}</td><td><button class="btn sm danger" data-del="${t.id}">删除</button></td></tr>`).join('')}
      </tbody></table></div>` : '<p class="small faint">还没有令牌</p>';
  } catch (e) { $('#tlist').innerHTML = `<div class="notice bad">${esc(e.message)}</div>`; }
}

// ---------------------------------------------------------------- avatar

function showAvatar(u) {
  const pending = u.avatar_pending && u.avatar_pending.length;
  $('#avbox').innerHTML = avatar(pending ? u.avatar_pending : u.avatar, u.nickname, 64);
  $('#avclear').hidden = !(u.avatar && u.avatar.length) && !pending;
  $('#avnote').textContent = pending ? '新头像正在等待审核，通过前别人看到的还是原来的。' : '显示在页头、你的评论和公开昵称的资料页上。图片会裁成正方形。';
}

/** Crops the picture to a 256×256 square in the browser, uploads it like any file (to
 * GitHub, never kept on this server) and makes it the avatar. Resolves to the new `me`. */
async function uploadAvatar(file) {
  const img = await createImageBitmap(file).catch(() => { throw new Error('读不出这张图片，换一张试试'); });
  const side = Math.min(img.width, img.height);
  const canvas = document.createElement('canvas');
  canvas.width = canvas.height = 256;
  canvas.getContext('2d').drawImage(img, (img.width - side) / 2, (img.height - side) / 2, side, side, 0, 0, 256, 256);
  const encode = (type) => new Promise((res) => canvas.toBlob(res, type, 0.86));
  let blob = await encode('image/webp');
  if (!blob || blob.type !== 'image/webp') blob = await encode('image/jpeg');
  const sha256 = [...new Uint8Array(await crypto.subtle.digest('SHA-256', await blob.arrayBuffer()))].map((b) => b.toString(16).padStart(2, '0')).join('');
  const ext = blob.type === 'image/webp' ? 'webp' : 'jpg';
  const plan = await api('/uploads', { method: 'POST', body: { filename: `avatar.${ext}`, mime: blob.type, parts: [{ size: blob.size, sha256 }] } });
  if (!plan.dedup) {
    let target = plan.parts[0].target;
    for (let attempt = 0; ; attempt++) {
      try {
        const receipt = await sendPart(target, blob, () => {});
        await api(`/uploads/${plan.upload_id}/parts/0`, { method: 'POST', body: { asset_id: receipt.id ?? null } });
        break;
      } catch (err) {
        if (attempt >= 1) throw err;
        // A failure on the upload Worker (another site) retries through this server's relay.
        const viaRelay = !target.url.startsWith('/');
        target = (await api(`/uploads/${plan.upload_id}/parts/0/renew${viaRelay ? '?via=relay' : ''}`, { method: 'POST' })).target;
      }
    }
  }
  return api('/me/avatar', { method: 'POST', body: { upload_id: plan.upload_id } });
}
