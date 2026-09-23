import { ago, api, esc, fmtDate, fmtSize, LEVELS, layout, loginUrl, pathText, resourceItem, toast, $ } from '../app.js';

let me;

/** A reviewable list of resources with per-item and bulk actions. */
function queueUI(box, items, intro, actions) {
  if (!items.length) {
    box.innerHTML = '<div class="card empty"><b>这里是空的</b>辛苦了 ☕</div>';
    return;
  }
  box.innerHTML = `<section class="card"><p class="small muted">${intro}</p>
    <div class="bulkbar"><label class="small"><input type="checkbox" id="qall"> 全选</label>
      ${actions.map(([a, l, c]) => `<button class="btn sm ${c}" data-bulk="${a}">批量${l}</button>`).join('')}
      <span class="grow"></span><span class="small muted">共 ${items.length} 份</span></div>
    <div class="list">${items.map((r) => `<div data-id="${r.id}">${resourceItem(r)}
      <div class="row small" style="padding:0 4px 14px 58px;gap:8px">
        <input type="checkbox" class="qsel">
        <span class="faint">${esc(r.original_name || '')}</span>
        ${r.uploader ? `<span class="faint">· ${esc(r.uploader.nickname)}</span>` : ''}<span class="faint">· ${ago(r.created_at)}</span>
        <span class="grow"></span>
        <input class="input" placeholder="备注" style="max-width:200px;min-height:30px;padding:3px 8px">
        ${actions.map(([a, l, c]) => `<button class="btn sm ${c}" data-a="${a}">${l}</button>`).join('')}
        <a class="btn sm" href="/r/${r.id}" target="_blank">打开</a>
      </div></div>`).join('')}</div></section>`;
  const act = async (wrap, action) => {
    await api(`/resources/${wrap.dataset.id}/review`, { method: 'POST', body: { action, note: wrap.querySelector('input.input').value } });
    wrap.remove();
  };
  box.onclick = async (e) => {
    const one = e.target.closest('[data-a]');
    const bulk = e.target.closest('[data-bulk]');
    try {
      if (one) { await act(one.closest('[data-id]'), one.dataset.a); toast('已处理'); }
      if (bulk) {
        const picked = [...box.querySelectorAll('.qsel:checked')].map((c) => c.closest('[data-id]'));
        if (!picked.length) return toast('先勾选资料', true);
        bulk.disabled = true;
        for (const w of picked) await act(w, bulk.dataset.bulk);
        bulk.disabled = false;
        toast(`已处理 ${picked.length} 份`);
      }
    } catch (err) { toast(err.message, true); }
  };
  box.querySelector('#qall').onchange = (e) => box.querySelectorAll('.qsel').forEach((c) => { c.checked = e.target.checked; });
}

const panels = {
  async queue(box) {
    queueUI(box, await api('/review'), '「待审核」来自贡献者，通过后才会公开；「待复核」来自可信贡献者，已公开。',
      [['approve', '通过', 'ok'], ['reject', '驳回', 'danger']]);
  },
  async uncertain(box) {
    queueUI(box, await api('/review?status=pending&uncertain=true'), '导入时标注「不确定」的资料（扫描件或压缩包，分类未经内容核实）。打开确认后通过，或改分类后再通过。',
      [['approve', '确认并公开', 'ok'], ['restrict', '设为仅内部', ''], ['reject', '驳回', 'danger']]);
  },
  async restricted(box) {
    queueUI(box, await api('/review?status=restricted'), '「仅内部」资料不公开、不可搜索（勿外传、加密题库等）。确认获得授权后可以恢复发布。',
      [['restore', '恢复发布', 'ok']]);
  },
  async reports(box) {
    const list = await api('/admin/reports');
    if (!list.length) { box.innerHTML = '<div class="card empty"><b>没有待处理的投诉</b></div>'; return; }
    box.innerHTML = `<section class="card"><p class="small muted">请在 48 小时内处理。需要下架的，先点「下架」再标记已处理。</p>${list.map((r) => `
      <div class="item" data-id="${r.id}" data-res="${r.resource ? r.resource.id : ''}"><div class="body">
        <div><b>${esc(r.reason)}</b></div>
        <div class="meta"><span>${ago(r.created_at)}</span>${r.contact ? `<span>联系：${esc(r.contact)}</span>` : ''}
          ${r.resource ? `<a href="/r/${r.resource.id}" target="_blank">${esc(r.resource.title)}</a><span class="badge ${esc(r.resource.status)}">${esc(r.resource.status)}</span>` : '<span>资料已不存在</span>'}</div>
        <div class="row" style="margin-top:8px"><input class="input" placeholder="处理说明" style="max-width:260px;min-height:30px;padding:3px 8px">
          ${r.resource ? '<button class="btn sm danger" data-a="remove">下架</button>' : ''}<button class="btn sm ok" data-a="done">标记已处理</button></div>
      </div></div>`).join('')}</section>`;
    box.onclick = async (e) => {
      const b = e.target.closest('[data-a]');
      if (!b) return;
      const it = b.closest('[data-id]');
      const note = it.querySelector('input').value;
      try {
        if (b.dataset.a === 'remove') { await api(`/resources/${it.dataset.res}/review`, { method: 'POST', body: { action: 'remove', note: note || '收到投诉，已下架' } }); toast('已下架'); }
        else { await api(`/admin/reports/${it.dataset.id}/handle`, { method: 'POST', body: { note } }); it.remove(); toast('已处理'); }
      } catch (err) { toast(err.message, true); }
    };
  },
  async nodes(box) {
    const list = await api('/review/nodes');
    if (!list.length) { box.innerHTML = '<div class="card empty"><b>没有待确认的分类</b></div>'; return; }
    box.innerHTML = `<section class="card scroll-x"><p class="small muted">上传者新建的课程。和已有课程重复的请合并过去。</p>
      <table class="table"><thead><tr><th>ID</th><th>位置</th><th>名称</th><th>资料</th><th></th></tr></thead><tbody>
      ${list.map(({ node: n, path }) => `<tr data-id="${n.id}"><td class="mono">${n.id}</td><td class="small faint">${esc(pathText(path))}</td>
        <td><input class="input" data-f="name" value="${esc(n.name)}" style="min-height:30px"></td><td>${n.count}</td>
        <td><div class="row" style="flex-wrap:nowrap"><button class="btn sm ok" data-a="ok">确认</button>
          <input class="input" data-f="into" placeholder="合并到ID" style="min-height:30px;max-width:90px">
          <button class="btn sm danger" data-a="merge">合并</button><a class="btn sm" href="/n/${n.id}" target="_blank">查看</a></div></td></tr>`).join('')}
      </tbody></table></section>`;
    box.onclick = async (e) => {
      const b = e.target.closest('[data-a]');
      if (!b) return;
      const tr = b.closest('tr');
      const f = (k) => tr.querySelector(`[data-f="${k}"]`).value;
      try {
        if (b.dataset.a === 'ok') await api(`/nodes/${tr.dataset.id}`, { method: 'PATCH', body: { name: f('name'), approve: true } });
        else {
          const into = Number(f('into'));
          if (!into) return toast('填写目标分类 ID', true);
          await api(`/nodes/${tr.dataset.id}/merge`, { method: 'POST', body: { into } });
        }
        tr.remove();
        toast('已处理');
      } catch (err) { toast(err.message, true); }
    };
  },
  async users(box, q = '') {
    const list = await api(`/admin/users?q=${encodeURIComponent(q)}`);
    const maxLevel = me.level === 4 ? 4 : 2;
    const opts = (cur) => [1, 2, 3, 4].filter((l) => l <= maxLevel || l === cur).map((l) => `<option value="${l}"${l === cur ? ' selected' : ''}${l > maxLevel ? ' disabled' : ''}>${LEVELS[l]}</option>`).join('');
    box.innerHTML = `<section class="card scroll-x">
      <form class="row" id="uq" style="margin-bottom:12px"><input class="input" id="uqv" value="${esc(q)}" placeholder="按邮箱或昵称搜索" style="max-width:280px"><button class="btn">搜索</button>
        <span class="small muted">贡献者先审后发；可信贡献者先发后审。${me.level < 4 ? '审核员只能调整贡献者和可信贡献者。' : ''}</span></form>
      <table class="table"><thead><tr><th>昵称</th><th>邮箱</th><th>角色</th><th>上传</th><th>注册</th><th></th></tr></thead><tbody>
      ${list.map((u) => `<tr data-id="${u.id}"><td>${esc(u.nickname)}</td><td class="small">${esc(u.email)}${u.xmu ? ' <span class="badge published">厦大</span>' : ''}</td>
        <td>${u.id === me.id ? LEVELS[u.level] : `<select class="input" data-f="level" style="min-height:30px;padding:2px 8px">${opts(u.level)}</select>`}</td>
        <td>${u.uploads}</td><td class="small faint">${fmtDate(u.created_at)}</td>
        <td>${u.id === me.id ? '<span class="small faint">你自己</span>' : `<button class="btn sm ${u.banned ? '' : 'danger'}" data-a="ban">${u.banned ? '解除封禁' : '封禁'}</button>`}</td></tr>`).join('')}
      </tbody></table></section>`;
    box.querySelector('#uq').onsubmit = (e) => { e.preventDefault(); panels.users(box, box.querySelector('#uqv').value); };
    box.onchange = async (e) => {
      const sel = e.target.closest('[data-f="level"]');
      if (!sel) return;
      try { await api(`/admin/users/${sel.closest('tr').dataset.id}`, { method: 'PATCH', body: { level: Number(sel.value) } }); toast('角色已更新'); } catch (err) { toast(err.message, true); }
    };
    box.onclick = async (e) => {
      const b = e.target.closest('[data-a="ban"]');
      if (!b) return;
      const banned = b.textContent === '封禁';
      try {
        await api(`/admin/users/${b.closest('tr').dataset.id}`, { method: 'PATCH', body: { banned } });
        b.textContent = banned ? '解除封禁' : '封禁';
        b.classList.toggle('danger', !banned);
      } catch (err) { toast(err.message, true); }
    };
  },
  async status(box) {
    const s = await api('/admin/status');
    const st = s.stats;
    box.innerHTML = `<section class="card"><h3>概况</h3><dl class="kv">
        <dt>已发布资料</dt><dd>${st.resources}</dd><dt>待审</dt><dd>${st.pending}</dd><dt>待处理投诉</dt><dd>${st.reports}</dd>
        <dt>用户</dt><dd>${st.users}</dd><dt>存储总量</dt><dd>${fmtSize(st.stored_bytes)}</dd><dt>下载次数</dt><dd>${st.downloads}</dd>
        <dt>今日中转</dt><dd>${s.relay_bytes_today != null ? fmtSize(s.relay_bytes_today) : '—'}</dd>
        <dt>邮件</dt><dd>${s.mail ? '已开启' : '<span class="badge rejected">未配置</span>'}</dd>
        <dt>内存占用</dt><dd>${s.rss_bytes ? fmtSize(s.rss_bytes) : '—'}</dd><dt>版本</dt><dd>${esc(s.version)}</dd></dl></section>
      <section class="card scroll-x"><h3>下载镜像</h3><p class="small muted">每 15 分钟从服务器经各镜像下载一个 256KB 探针文件，按实际速度排序；下载时依次尝试，全部失败时直连 GitHub。</p>
        ${s.mirrors.length ? `<table class="table"><thead><tr><th>镜像</th><th>状态</th><th>速度</th><th>检测时间</th></tr></thead><tbody>
        ${s.mirrors.map((m) => `<tr><td class="mono">${esc(m.prefix)}</td><td>${m.ok ? '<span class="badge published">可用</span>' : `<span class="badge rejected">不可用</span> <span class="small faint">${esc(m.error)}</span>`}</td>
          <td>${m.ok ? `${(m.speed_kbps / 1024).toFixed(2)} MB/s` : '—'}</td><td class="small faint">${ago(m.checked_at)}</td></tr>`).join('')}</tbody></table>` : '<p class="faint">尚未探测（本地存储模式或刚启动）</p>'}
      </section>`;
  },
};

async function show(name) {
  document.querySelectorAll('#tabs button').forEach((b) => b.classList.toggle('on', b.dataset.t === name));
  const box = $('#panel');
  box.onclick = box.onchange = null;
  box.innerHTML = '<div class="skeleton"></div>';
  try {
    await panels[name](box);
  } catch (e) {
    box.innerHTML = `<div class="notice bad">${esc(e.message)}</div>`;
  }
}

(async () => {
  me = await layout('admin');
  if (!me) { location.replace(loginUrl()); return; }
  if (me.level < 3) {
    $('#tabs').hidden = true;
    $('#panel').innerHTML = '<div class="card empty"><b>需要审核员或管理员权限</b></div>';
    return;
  }
  $('#who').textContent = `${LEVELS[me.level]} · ${me.nickname}`;
  $('#tabs').onclick = (e) => { const b = e.target.closest('button'); if (b) show(b.dataset.t); };
  show('queue');
})();
