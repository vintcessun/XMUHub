import { ago, api, esc, fmtDate, fmtSize, LEVELS, layout, resourceItem, toast, $ } from '../app.js';

const mePromise = layout('admin');
let me;

const panels = {
  async queue(box) {
    const q = await api('/review');
    if (!q.resources.length) {
      box.innerHTML = '<div class="card empty"><b>审核队列是空的</b>辛苦了 ☕</div>';
      return;
    }
    box.innerHTML = `<section class="card"><p class="small muted">共 ${q.resources.length} 份：「待审核」来自贡献者，通过后才会公开；「待复核」来自可信贡献者，已公开。</p>
      <div class="list">${q.resources.map((r) => `<div data-id="${r.id}">${resourceItem(r)}
        <div class="row" style="padding:0 4px 14px 58px">
          <input class="input" placeholder="备注（驳回原因）" style="max-width:280px;min-height:32px;padding:4px 10px">
          <button class="btn sm ok" data-a="approve">通过</button>
          <button class="btn sm danger" data-a="${r.status === 'pending' ? 'reject' : 'remove'}">${r.status === 'pending' ? '驳回' : '下架'}</button>
          <a class="btn sm" href="/r/${r.id}" target="_blank">预览/下载</a>
          <span class="small faint">${ago(r.created_at)}</span>
        </div></div>`).join('')}</div></section>`;
    box.onclick = async (e) => {
      const b = e.target.closest('[data-a]');
      if (!b) return;
      const wrap = b.closest('[data-id]');
      try {
        await api(`/resources/${wrap.dataset.id}/review`, { method: 'POST', body: { action: b.dataset.a, note: wrap.querySelector('input').value } });
        wrap.remove();
        toast('已处理');
      } catch (err) { toast(err.message, true); }
    };
  },

  async courses(box) {
    const q = await api('/review');
    if (!q.courses.length) {
      box.innerHTML = '<div class="card empty"><b>没有待确认的课程</b></div>';
      return;
    }
    box.innerHTML = `<section class="card scroll-x"><p class="small muted">上传时新建的课程。确认前请检查是否和已有课程重复——重复的请「合并」到已有课程。</p>
      <table class="table"><thead><tr><th>ID</th><th>名称</th><th>代码</th><th>学院</th><th>资料</th><th></th></tr></thead><tbody>
      ${q.courses.map((c) => `<tr data-id="${c.id}"><td class="mono">${c.id}</td>
        <td><input class="input" data-f="name" value="${esc(c.name)}" style="min-height:32px"></td>
        <td><input class="input" data-f="code" value="${esc(c.code)}" style="min-height:32px;max-width:120px"></td>
        <td><input class="input" data-f="college" value="${esc(c.college)}" style="min-height:32px"></td>
        <td>${c.count}</td>
        <td><div class="row" style="flex-wrap:nowrap"><button class="btn sm ok" data-a="ok">确认</button>
          <input class="input" data-f="into" placeholder="合并到ID" style="min-height:32px;max-width:90px">
          <button class="btn sm danger" data-a="merge">合并</button><a class="btn sm" href="/c/${c.id}" target="_blank">查看</a></div></td></tr>`).join('')}
      </tbody></table></section>`;
    box.onclick = async (e) => {
      const b = e.target.closest('[data-a]');
      if (!b) return;
      const tr = b.closest('tr');
      const f = (k) => tr.querySelector(`[data-f="${k}"]`).value;
      try {
        if (b.dataset.a === 'ok') {
          await api(`/courses/${tr.dataset.id}`, { method: 'PATCH', body: { name: f('name'), code: f('code'), college: f('college'), approve: true } });
        } else {
          const into = Number(f('into'));
          if (!into) return toast('填写目标课程 ID', true);
          await api(`/courses/${tr.dataset.id}/merge`, { method: 'POST', body: { into } });
        }
        tr.remove();
        toast('已处理');
      } catch (err) { toast(err.message, true); }
    };
  },

  async tokens(box) {
    const list = await api('/admin/tokens');
    const maxLevel = me.level === 4 ? 4 : 2;
    const levelOpts = (cur) => [1, 2, 3, 4].filter((l) => l <= maxLevel).map((l) => `<option value="${l}"${l === cur ? ' selected' : ''}>${LEVELS[l]}</option>`).join('');
    box.innerHTML = `<section class="card"><h3>签发令牌</h3>
        <form class="row" id="issue"><select class="input" id="lv" style="max-width:160px">${levelOpts(2)}</select>
          <input class="input" id="lb" placeholder="备注，如 张同学 / 数院资料组" style="max-width:280px"><button class="btn primary">签发</button></form>
        <div id="issued"></div></section>
      <section class="card scroll-x"><h3>全部令牌（${list.length}）</h3>
        <table class="table"><thead><tr><th>ID</th><th>级别</th><th>备注</th><th>上传</th><th>领取</th><th>状态</th></tr></thead><tbody>
        ${list.map((t) => `<tr data-id="${t.id}"><td class="mono">${t.id}</td>
          <td>${t.id === me.id ? LEVELS[t.level] : `<select class="input" data-f="level" style="min-height:30px;padding:2px 8px">${levelOpts(t.level)}</select>`}</td>
          <td>${esc(t.label)}</td><td>${t.uploads}</td>
          <td class="small faint">${fmtDate(t.created_at)}<br>${esc(t.created_ip)}</td>
          <td>${t.id === me.id ? '<span class="small faint">你自己</span>' : `<button class="btn sm ${t.banned ? '' : 'danger'}" data-a="ban">${t.banned ? '解除停用' : '停用'}</button>`}</td></tr>`).join('')}
        </tbody></table></section>`;
    $('#issue').onsubmit = async (e) => {
      e.preventDefault();
      try {
        const r = await api('/admin/tokens', { method: 'POST', body: { level: Number($('#lv').value), label: $('#lb').value } });
        $('#issued').innerHTML = `<p class="notice ok" style="margin-top:12px">已签发 ${LEVELS[r.info.level]} 令牌，<b>只显示这一次</b>，请发给对方：</p><div class="secret">${esc(r.token)}</div>`;
      } catch (err) { toast(err.message, true); }
    };
    box.onchange = async (e) => {
      const sel = e.target.closest('[data-f="level"]');
      if (!sel) return;
      try {
        await api(`/admin/tokens/${sel.closest('tr').dataset.id}`, { method: 'PATCH', body: { level: Number(sel.value) } });
        toast('级别已更新');
      } catch (err) { toast(err.message, true); }
    };
    box.onclick = async (e) => {
      const b = e.target.closest('[data-a="ban"]');
      if (!b) return;
      const banned = b.textContent === '停用';
      try {
        await api(`/admin/tokens/${b.closest('tr').dataset.id}`, { method: 'PATCH', body: { banned } });
        b.textContent = banned ? '解除停用' : '停用';
        b.classList.toggle('danger', !banned);
      } catch (err) { toast(err.message, true); }
    };
  },

  async status(box) {
    const s = await api('/admin/status');
    const st = s.stats;
    box.innerHTML = `<section class="card"><h3>概况</h3><dl class="kv">
        <dt>课程</dt><dd>${st.courses}</dd><dt>已发布资料</dt><dd>${st.resources}</dd><dt>待审</dt><dd>${st.pending}</dd>
        <dt>令牌</dt><dd>${st.tokens}</dd><dt>存储总量</dt><dd>${fmtSize(st.stored_bytes)}</dd><dt>下载次数</dt><dd>${st.downloads}</dd>
        <dt>内存占用</dt><dd>${s.rss_bytes ? fmtSize(s.rss_bytes) : '—'}</dd><dt>版本</dt><dd>${esc(s.version)}</dd></dl></section>
      <section class="card scroll-x"><h3>下载镜像</h3><p class="small muted">每 20 分钟从服务器探测一次，下载时按延迟从低到高依次尝试；全部失败时直连 GitHub。</p>
        ${s.mirrors.length ? `<table class="table"><thead><tr><th>镜像</th><th>状态</th><th>延迟</th><th>检测时间</th></tr></thead><tbody>
        ${s.mirrors.map((m) => `<tr><td class="mono">${esc(m.prefix)}</td><td>${m.ok ? '<span class="badge published">可用</span>' : `<span class="badge rejected">不可用</span> <span class="small faint">${esc(m.error)}</span>`}</td>
          <td>${m.latency_ms} ms</td><td class="small faint">${ago(m.checked_at)}</td></tr>`).join('')}</tbody></table>` : '<p class="faint">尚未探测（本地存储模式或刚启动）</p>'}
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
  me = await mePromise;
  if (me.level < 3) {
    $('#tabs').hidden = true;
    $('#panel').innerHTML = '<div class="card empty"><b>需要审核员或管理员令牌</b>在「我的」页面粘贴令牌后再来</div>';
    return;
  }
  $('#who').textContent = `${LEVELS[me.level]}${me.label ? ' · ' + me.label : ''}`;
  $('#tabs').onclick = (e) => { const b = e.target.closest('button'); if (b) show(b.dataset.t); };
  show('queue');
})();
