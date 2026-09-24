import { ago, api, esc, fmtDate, fmtSize, LEVELS, layout, loginUrl, meta, modal, moveResources, pathText, preview, resourceItem, toast, tree, $ } from '../app.js';

let me;

/** 类型 picker grouped by tag: choosing a word sets both the name's type word and the tag. */
function typeSelect(M, r) {
  const cur = r.name.type_word;
  const groups = M.tags.map((t) => {
    const words = M.type_words.filter((w) => w.tag === t.code);
    return words.length ? `<optgroup label="${t.code} ${t.label}">${words.map((w) => `<option value="${w.word}|${t.code}"${w.word === cur && t.code === r.tag.code ? ' selected' : ''}>${w.word}</option>`).join('')}</optgroup>` : '';
  }).join('');
  const known = M.type_words.some((w) => w.word === cur && w.tag === r.tag.code);
  return `<select class="input" data-f="type" title="类型 / 标签" style="max-width:150px;min-height:30px;padding:2px 6px">${known ? '' : `<option value="${esc(cur)}|${r.tag.code}" selected>${esc(cur)}（${r.tag.label}）</option>`}${groups}</select>`;
}

/** Changes a resource's type word + tag, keeping every other name part. */
async function retype(r, value) {
  const [type_word, tag] = value.split('|');
  const n = r.name;
  return api(`/resources/${r.id}`, {
    method: 'PATCH',
    body: {
      node: r.node.id, course: n.course, time: n.time, type_word, tag, paper: n.paper, with_answer: n.with_answer,
      extra: n.extra, note: r.note, admin: { uncertain: !!r.uncertain, free_type: true },
    },
  });
}

/** A reviewable list of resources with per-item and bulk actions. */
async function queueUI(box, items, intro, actions) {
  if (!items.length) {
    box.innerHTML = '<div class="card empty"><b>这里是空的</b>辛苦了 ☕</div>';
    return;
  }
  const M = await meta();
  const byId = new Map(items.map((r) => [String(r.id), r]));
  box.innerHTML = `<section class="card"><p class="small muted">${intro}</p>
    <div class="bulkbar"><label class="small"><input type="checkbox" id="qall"> 全选</label>
      ${actions.map(([a, l, c]) => `<button class="btn sm ${c}" data-bulk="${a}">批量${l}</button>`).join('')}
      <button class="btn sm" data-move type="button">批量移动分类</button>
      <span class="grow"></span><span class="small muted">共 ${items.length} 份</span></div>
    <div class="list">${items.map((r) => `<div data-id="${r.id}">${resourceItem(r)}
      <div class="row small" style="padding:0 4px 14px 58px;gap:8px">
        <input type="checkbox" class="qsel">
        <span class="faint">${esc(r.original_name || '')}</span>
        ${r.uploader ? `<span class="faint">· ${esc(r.uploader.nickname)}</span>` : ''}<span class="faint">· ${ago(r.created_at)}</span>
        <span class="grow"></span>
        ${typeSelect(M, r)}
        <input class="input" data-f="note" placeholder="备注" style="max-width:180px;min-height:30px;padding:3px 8px">
        <button class="btn sm" data-pv type="button">预览</button>
        ${actions.map(([a, l, c]) => `<button class="btn sm ${c}" data-a="${a}">${l}</button>`).join('')}
        <a class="btn sm" href="/r/${r.id}" target="_blank">打开</a>
      </div></div>`).join('')}</div></section>`;
  const act = async (wrap, action) => {
    await api(`/resources/${wrap.dataset.id}/review`, { method: 'POST', body: { action, note: wrap.querySelector('[data-f="note"]').value } });
    wrap.remove();
  };
  box.onchange = async (e) => {
    const sel = e.target.closest('[data-f="type"]');
    if (!sel) return;
    const wrap = sel.closest('[data-id]');
    try {
      const r = await retype(byId.get(wrap.dataset.id), sel.value);
      byId.set(String(r.id), r);
      wrap.querySelector('.title').textContent = r.title;
      const tagEl = wrap.querySelector('.tag');
      tagEl.className = `tag t-${r.tag.code}`;
      tagEl.textContent = r.tag.label;
      toast(`已改为 ${r.name.type_word}（${r.tag.label}）`);
    } catch (err) { toast(err.message, true); }
  };
  box.onclick = async (e) => {
    const pv = e.target.closest('[data-pv]');
    if (pv) {
      const r = byId.get(pv.closest('[data-id]').dataset.id);
      preview(r.id, modal(r.title, { wide: true }));
      return;
    }
    if (e.target.closest('[data-move]')) {
      const picked = [...box.querySelectorAll('.qsel:checked')].map((c) => c.closest('[data-id]'));
      if (await moveResources(picked.map((w) => Number(w.dataset.id)))) show(current);
      return;
    }
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

/** Bar + line chart as inline SVG: bars for daily counts, a line for the running total. */
function chart(days, bars, line) {
  const W = 760, H = 240, L = 40, R = 46, T = 12, B = 28;
  const w = (W - L - R) / days.length;
  const maxBar = Math.max(1, ...days.flatMap((d) => bars.map(([k]) => d[k])));
  const maxLine = line ? Math.max(1, ...days.map((d) => d[line[0]])) : 1;
  const minLine = line ? Math.min(...days.map((d) => d[line[0]])) : 0;
  const y = (v) => T + (H - T - B) * (1 - v / maxBar);
  const yl = (v) => T + (H - T - B) * (1 - (v - minLine * 0.9) / Math.max(1, maxLine - minLine * 0.9));
  const ticks = [0, 0.5, 1].map((f) => Math.round(maxBar * f));
  const bw = Math.max(1, (w - 2) / bars.length);
  let svg = ticks.map((t) => `<line class="grid" x1="${L}" x2="${W - R}" y1="${y(t)}" y2="${y(t)}"/>${bars.length ? `<text x="${L - 6}" y="${y(t) + 4}" text-anchor="end">${t}</text>` : ''}`).join('');
  days.forEach((d, i) => {
    bars.forEach(([k, cls], j) => {
      const v = d[k];
      if (v) svg += `<rect class="${cls}" x="${L + i * w + 1 + j * bw}" y="${y(v)}" width="${bw}" height="${H - B - y(v)}"><title>${d.day} ${v}</title></rect>`;
    });
    if (i % Math.ceil(days.length / 8) === 0) svg += `<text x="${L + i * w + w / 2}" y="${H - 8}" text-anchor="middle">${d.day.slice(5)}</text>`;
  });
  if (line) {
    svg += `<polyline class="line" points="${days.map((d, i) => `${L + i * w + w / 2},${yl(d[line[0]])}`).join(' ')}"/>`;
    svg += `<text x="${W - R + 4}" y="${yl(maxLine) + 4}">${maxLine}</text><text x="${W - R + 4}" y="${yl(minLine) + 4}">${minLine}</text>`;
  }
  return `<svg class="chart" viewBox="0 0 ${W} ${H}" role="img">${svg}</svg>`;
}

let current = 'dash';

const panels = {
  async dash(box, days = 30) {
    const [list, s] = await Promise.all([api(`/admin/daily?days=${days}`), api('/admin/status')]);
    const sum = (k) => list.reduce((n, d) => n + d[k], 0);
    const last = list[list.length - 1];
    const st = s.stats;
    box.innerHTML = `<section class="card"><div class="row" style="margin-bottom:12px"><h3 style="margin:0">概览</h3><span class="grow"></span>
        <select class="input" id="ddays" style="max-width:120px">${[14, 30, 90, 180].map((d) => `<option value="${d}"${d === days ? ' selected' : ''}>最近 ${d} 天</option>`).join('')}</select></div>
      <div class="kpis">
        <div class="kpi"><b>${st.resources}</b><span>已发布资料（总文件量）</span></div>
        <div class="kpi"><b>${st.pending}</b><span>待审 / 待复核</span></div>
        <div class="kpi"><b>${last.submitted}</b><span>今日提交</span></div>
        <div class="kpi"><b>${last.reviewed}</b><span>今日审核</span></div>
        <div class="kpi"><b>${sum('submitted')}</b><span>${days} 天提交</span></div>
        <div class="kpi"><b>${sum('reviewed')}</b><span>${days} 天审核</span></div>
        <div class="kpi"><b>${st.users}</b><span>注册用户</span></div>
        <div class="kpi"><b>${fmtSize(st.stored_bytes)}</b><span>存储总量</span></div>
      </div></section>
      <section class="card"><h3>每日提交量与审核量</h3>
        <div class="legend"><span><i style="background:var(--navy-2)"></i>提交</span><span><i style="background:var(--gold)"></i>审核</span></div>
        ${chart(list, [['submitted', 'bar-a'], ['reviewed', 'bar-b']])}</section>
      <section class="card"><h3>总文件量（已发布）</h3>${chart(list, [], ['total'])}
        <p class="small faint" style="margin:6px 0 0">审核量从审核记录上线（9 月 24 日）起统计；总文件量按资料的创建日期累计当前已发布的资料。</p></section>
      <section class="card scroll-x"><h3>明细</h3><table class="table"><thead><tr><th>日期</th><th>提交</th><th>审核</th><th>总文件量</th></tr></thead><tbody>
        ${list.slice().reverse().map((d) => `<tr><td>${d.day}</td><td>${d.submitted}</td><td>${d.reviewed}</td><td>${d.total}</td></tr>`).join('')}</tbody></table></section>`;
    box.querySelector('#ddays').onchange = (e) => panels.dash(box, Number(e.target.value));
  },
  async queue(box) {
    await queueUI(box, await api('/review'), '「待审核」来自贡献者，通过后才会公开；「待复核」来自可信贡献者，已公开。',
      [['approve', '通过', 'ok'], ['reject', '驳回', 'danger']]);
  },
  async uncertain(box) {
    await queueUI(box, await api('/review?status=pending&uncertain=true'), '导入时标注「不确定」的资料（扫描件或压缩包，分类未经内容核实）。打开确认后通过，或改分类后再通过。',
      [['approve', '确认并公开', 'ok'], ['restrict', '设为仅内部', ''], ['reject', '驳回', 'danger']]);
  },
  async restricted(box) {
    await queueUI(box, await api('/review?status=restricted'), '「仅内部」资料不公开、不可搜索（勿外传、加密题库等）。确认获得授权后可以恢复发布。',
      [['restore', '恢复发布', 'ok']]);
  },
  async reports(box, all = false) {
    const list = await api(`/admin/reports${all ? '?all=true' : ''}`);
    const STATUS = { pending: '待审核', published: '已发布', rejected: '未通过', removed: '已下架', restricted: '仅内部' };
    box.innerHTML = `<section class="card"><div class="row" style="margin-bottom:8px"><p class="small muted" style="margin:0">请在 48 小时内处理。需要下架的，先点「下架」再标记已处理。</p><span class="grow"></span>
        <label class="small"><input type="checkbox" id="rpall"${all ? ' checked' : ''}> 显示已处理</label></div>
      ${list.length ? list.map((r) => `
      <div class="item" data-id="${r.id}" data-res="${r.resource ? r.resource.id : ''}"><div class="body">
        <div><b>${esc(r.reason)}</b></div>
        <div class="meta"><span>${ago(r.created_at)}</span>${r.contact ? `<span>投诉人：${esc(r.contact)}</span>` : ''}
          ${r.resource ? `<a href="/r/${r.resource.id}" target="_blank">${esc(r.resource.title)}</a><span class="badge ${esc(r.resource.status)}">${STATUS[r.resource.status] || esc(r.resource.status)}</span>` : '<span>资料已不存在</span>'}
          ${r.handled ? `<span class="badge published">已处理${r.handled_by ? ` · ${esc(r.handled_by)}` : ''}</span>${r.handled_note ? `<span>${esc(r.handled_note)}</span>` : ''}` : ''}
          ${me.level >= 4 ? '<a href="#" class="small" data-a="delete" style="color:var(--bad)">删除</a>' : ''}</div>
        ${r.handled ? '' : `<div class="row" style="margin-top:8px"><input class="input" placeholder="处理说明" style="max-width:260px;min-height:30px;padding:3px 8px">
          ${r.resource && r.resource.status !== 'removed' ? '<button class="btn sm danger" data-a="remove">下架</button>' : ''}<button class="btn sm ok" data-a="done">标记已处理</button></div>`}
      </div></div>`).join('') : `<div class="empty"><b>${all ? '还没有投诉' : '没有待处理的投诉'}</b>${all ? '' : '勾选右上角「显示已处理」可查看历史'}</div>`}</section>`;
    box.querySelector('#rpall').onchange = (e) => panels.reports(box, e.target.checked);
    box.onclick = async (e) => {
      const b = e.target.closest('[data-a]');
      if (!b) return;
      const it = b.closest('[data-id]');
      const note = it.querySelector('input')?.value || '';
      try {
        if (b.dataset.a === 'delete') {
          e.preventDefault();
          if (!confirm('删除这条投诉记录？（只删记录，不影响资料）')) return;
          await api(`/admin/reports/${it.dataset.id}`, { method: 'DELETE' });
          toast('已删除');
          panels.reports(box, all);
          return;
        }
        if (b.dataset.a === 'remove') { await api(`/resources/${it.dataset.res}/review`, { method: 'POST', body: { action: 'remove', note: note || '收到投诉，已下架' } }); toast('已下架'); panels.reports(box, all); }
        else { await api(`/admin/reports/${it.dataset.id}/handle`, { method: 'POST', body: { note } }); toast('已处理，可在「显示已处理」中查看'); panels.reports(box, all); }
      } catch (err) { toast(err.message, true); }
    };
  },
  async feedback(box, all = false) {
    const list = await api(`/admin/feedback${all ? '?all=true' : ''}`);
    box.innerHTML = `<section class="card"><div class="row" style="margin-bottom:8px"><p class="small muted" style="margin:0">用户从「意见反馈」页提交的问题和建议。</p><span class="grow"></span>
        <label class="small"><input type="checkbox" id="fball"${all ? ' checked' : ''}> 显示已处理</label></div>
      ${list.length ? list.map((f) => `<div class="item" data-id="${f.id}"><div class="body">
        <div style="white-space:pre-wrap;overflow-wrap:anywhere">${esc(f.body)}</div>
        <div class="meta"><span>${ago(f.created_at)}</span>${f.nickname ? `<span>${esc(f.nickname)}</span>` : '<span>未登录</span>'}${f.contact ? `<span>联系：${esc(f.contact)}</span>` : ''}${f.page ? `<span class="faint">${esc(f.page)}</span>` : ''}
          ${f.handled ? `<span class="badge published">已处理${f.handled_by ? ` · ${esc(f.handled_by)}` : ''}</span>${f.handled_note ? `<span>${esc(f.handled_note)}</span>` : ''}` : ''}</div>
        ${f.handled ? '' : `<div class="row" style="margin-top:8px"><input class="input" placeholder="处理说明（选填）" style="max-width:260px;min-height:30px;padding:3px 8px"><button class="btn sm ok" data-a="done">标记已处理</button></div>`}
      </div></div>`).join('') : '<div class="empty"><b>没有待处理的反馈</b></div>'}</section>`;
    box.querySelector('#fball').onchange = (e) => panels.feedback(box, e.target.checked);
    box.onclick = async (e) => {
      const b = e.target.closest('[data-a="done"]');
      if (!b) return;
      const it = b.closest('[data-id]');
      try { await api(`/admin/feedback/${it.dataset.id}/handle`, { method: 'POST', body: { note: it.querySelector('input').value } }); toast('已处理'); panels.feedback(box, all); } catch (err) { toast(err.message, true); }
    };
  },
  async log(box) {
    const list = await api('/admin/reviews');
    const A = { approve: ['通过', 'published'], reject: ['驳回', 'rejected'], remove: ['下架', 'removed'], restrict: ['仅内部', 'restricted'], restore: ['恢复发布', 'published'] };
    box.innerHTML = `<section class="card scroll-x"><p class="small muted">最近 300 条审核操作（谁在什么时候通过、驳回或下架了哪份资料）。</p>
      ${list.length ? `<table class="table"><thead><tr><th>时间</th><th>审核人</th><th>操作</th><th>资料</th><th>备注</th></tr></thead><tbody>
      ${list.map((e) => `<tr><td class="small faint">${fmtDate(e.at, true)}</td><td>${esc(e.actor)}</td>
        <td><span class="badge ${(A[e.action] || [])[1] || ''}">${(A[e.action] || [e.action])[0]}</span></td>
        <td><a href="/r/${e.resource}" target="_blank">${esc(e.title || `#${e.resource}`)}</a></td><td class="small">${esc(e.note)}</td></tr>`).join('')}</tbody></table>`
      : '<div class="empty"><b>还没有审核记录</b>（记录从这次更新开始）</div>'}</section>`;
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
    // Admins come only from the maintained admin list; the UI can grant up to reviewer.
    const maxLevel = me.level === 4 ? 3 : 2;
    const opts = (cur) => [1, 2, 3].filter((l) => l <= maxLevel || l === cur).map((l) => `<option value="${l}"${l === cur ? ' selected' : ''}${l > maxLevel ? ' disabled' : ''}>${LEVELS[l]}</option>`).join('');
    box.innerHTML = `<section class="card scroll-x">
      <form class="row" id="uq" style="margin-bottom:12px"><input class="input" id="uqv" value="${esc(q)}" placeholder="按邮箱或昵称搜索" style="max-width:280px"><button class="btn">搜索</button>
        <span class="small muted">贡献者先审后发；可信贡献者先发后审。管理员由服务器上的管理员名单维护，这里最多设为审核员；同级之间不能互相封禁或调整。${me.level < 4 ? '审核员只能调整贡献者和可信贡献者。' : ''}</span></form>
      <table class="table"><thead><tr><th>昵称</th><th>邮箱</th><th>角色</th><th>上传</th><th>注册</th><th></th></tr></thead><tbody>
      ${list.map((u) => `<tr data-id="${u.id}"><td>${esc(u.nickname)}</td><td class="small">${esc(u.email)}${u.xmu ? ' <span class="badge published">厦大</span>' : ''}</td>
        <td>${u.id === me.id || u.level >= me.level ? `${LEVELS[u.level]}${u.level === 4 ? ' <span class="small faint">（名单）</span>' : ''}` : `<select class="input" data-f="level" style="min-height:30px;padding:2px 8px">${opts(u.level)}</select>`}</td>
        <td>${u.uploads}</td><td class="small faint">${fmtDate(u.created_at)}</td>
        <td>${u.id === me.id ? '<span class="small faint">你自己</span>' : u.level >= me.level ? '<span class="small faint">同级</span>' : `<button class="btn sm ${u.banned ? '' : 'danger'}" data-a="ban">${u.banned ? '解除封禁' : '封禁'}</button>`}</td></tr>`).join('')}
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
  async github(box) {
    if (me.level < 4) { box.innerHTML = '<div class="card empty"><b>仓库导入仅限管理员</b></div>'; return; }
    const t = await tree();
    const bSection = t.children(0).find((x) => x.code === 'B');
    const colleges = bSection ? t.children(bSection.id) : [];
    const tr = await api('/admin/github/transfer');
    box.innerHTML = `<section class="card">
        <h3>从 GitHub 仓库导入</h3>
        <p class="small muted">只导入文档和压缩包（pdf、doc、ppt、xls、epub、zip、rar、7z），代码文件跳过。导入时直接引用原仓库的文件（固定到当前 commit，经国内镜像下载），随后后台用 GitHub Actions 在 GitHub 内部转存到我们的仓库，全程不经过服务器。导入的资料全部进「待核实」。</p>
        <form class="row" id="gf"><input class="input grow" id="gurl" placeholder="https://github.com/owner/repo" style="min-width:260px">
          <select class="input" id="gdepth" style="max-width:150px"><option value="">自动分组</option><option value="1">按第 1 层目录</option><option value="2">按第 2 层目录</option><option value="3">按第 3 层目录</option></select>
          <button class="btn primary">扫描</button></form>
        <div id="gres"></div></section>
      <section class="card"><h3>后台转存</h3>
        <p class="small">仅引用：<b>${tr.referenced}</b> 个文件 · 已有自己的副本：<b>${tr.owned}</b> 个${tr.current ? ` · 正在运行批次 ${esc(tr.current.id)}（${tr.current.files} 个文件，${ago(tr.current.dispatched_at)}开始）` : ''}</p>
        <p class="small muted">每 5 分钟检查一次，每批最多 400 个文件、6GB。</p><button class="btn sm" id="gkick">立即检查</button></section>`;
    box.querySelector('#gkick').onclick = async () => {
      try { await api('/admin/github/transfer', { method: 'POST' }); toast('已触发'); show('github'); } catch (e) { toast(e.message, true); }
    };
    const ancestors = (n) => { const out = []; let p = n.parent; while (p) { const x = t.byId.get(p); if (!x) break; out.unshift(x); p = x.parent; } return out; };
    const label = (id) => { const n = t.byId.get(id); return n ? `${n.name}（${pathText(ancestors(n))}）` : `#${id}`; };
    // Preselect only confident matches; pick the 上/下 level when the folder names one.
    const pick = (g) => {
      const leaf = g.key.split('/').pop();
      const top = g.suggest[0];
      if (!top) return null;
      const bare = leaf.replace(/[（(].*?[)）]/g, '').trim();
      const exact = [top.node.name, top.node.label].some((n) => n === leaf || n === bare) || top.node.code === leaf;
      if (!exact) return null;
      const lv = /[（(]\s*([上下])\s*[)）]/.exec(leaf);
      if (lv) { const child = t.children(top.node.id).find((c) => c.name === lv[1]); if (child) return child.id; }
      return top.node.id;
    };
    box.querySelector('#gf').onsubmit = async (e) => {
      e.preventDefault();
      const res = box.querySelector('#gres');
      res.innerHTML = '<p class="muted small">正在扫描仓库目录（大仓库需要十几秒）…</p>';
      let scan;
      try {
        scan = await api('/admin/github/scan', { method: 'POST', body: { url: box.querySelector('#gurl').value, depth: Number(box.querySelector('#gdepth').value) || null } });
      } catch (err) { res.innerHTML = `<div class="notice bad">${esc(err.message)}</div>`; return; }
      res.innerHTML = `<p class="small" style="margin-top:12px"><b>${esc(scan.owner)}/${esc(scan.repo)}</b> · ${esc(scan.branch)} @ <span class="mono">${esc(scan.commit.slice(0, 10))}</span> · 许可证 ${esc(scan.license || '未声明')} ·
          共 ${scan.total_files} 个文件，其中文档 <b>${scan.doc_files}</b> 个（${fmtSize(scan.doc_bytes)}），按第 ${scan.depth} 层目录分成 ${scan.groups.length} 组</p>
        <div class="bulkbar"><span class="small">「新建课程」默认放在：</span><select class="input" id="gcol">${colleges.map((c) => `<option value="${c.id}"${c.name === '信息学院' ? ' selected' : ''}>${esc(c.name)}</option>`).join('')}</select>
          <span class="grow"></span><span class="small muted">没选分类的组不导入</span></div>
        <div class="scroll-x"><table class="table"><thead><tr><th>目录</th><th>文件</th><th>导入到分类</th><th></th></tr></thead><tbody>
        ${scan.groups.map((g, i) => {
          const chosen = pick(g);
          const opts = g.suggest.map((x) => `<option value="${x.node.id}"${x.node.id === chosen ? ' selected' : ''}>${esc(x.node.name)}（${esc(pathText(x.path))}）</option>`).join('');
          const extra = chosen && !g.suggest.some((x) => x.node.id === chosen) ? `<option value="${chosen}" selected>${esc(label(chosen))}</option>` : '';
          return `<tr data-i="${i}"><td><b>${esc(g.key || '（根目录）')}</b><div class="small faint">${g.samples.map(esc).join('、')}</div></td>
            <td class="small">${g.files} · ${fmtSize(g.bytes)}</td>
            <td><select class="input" data-f="node" style="min-height:30px;max-width:340px"><option value="">— 不导入 —</option>${extra}${opts}</select>
              <input class="input" data-f="id" placeholder="或填分类 ID" style="min-height:30px;max-width:120px;margin-top:4px"></td>
            <td><button class="btn sm" data-a="new" type="button">新建课程</button></td></tr>`;
        }).join('')}</tbody></table></div>
        <div class="row" style="margin-top:12px"><button class="btn primary" id="gimp" type="button">导入已选择的组</button><span class="small muted" id="gsum"></span></div>`;
      const rows = [...res.querySelectorAll('tr[data-i]')];
      const chosenOf = (row) => Number(row.querySelector('[data-f="id"]').value) || Number(row.querySelector('[data-f="node"]').value) || 0;
      const sum = () => {
        const sel = rows.filter((r) => chosenOf(r));
        const files = sel.reduce((n, r) => n + scan.groups[Number(r.dataset.i)].files, 0);
        res.querySelector('#gsum').textContent = `已选 ${sel.length} / ${rows.length} 组，${files} 个文件`;
      };
      sum();
      res.onchange = sum;
      res.oninput = sum;
      res.onclick = async (ev) => {
        const b = ev.target.closest('[data-a="new"]');
        if (!b) return;
        const row = b.closest('tr');
        const g = scan.groups[Number(row.dataset.i)];
        const name = prompt('新建课程名称（教务全称）：', g.key.split('/').pop().replace(/[（(].*?[)）]/g, '').trim());
        if (!name) return;
        try {
          const n = await api('/nodes', { method: 'POST', body: { parent: Number(res.querySelector('#gcol').value), kind: 'course', name } });
          row.querySelector('[data-f="id"]').value = n.id;
          toast(`已新建「${n.name}」`);
          sum();
        } catch (err) { toast(err.message, true); }
      };
      res.querySelector('#gimp').onclick = async () => {
        const mappings = {};
        for (const r of rows) { const id = chosenOf(r); if (id) mappings[scan.groups[Number(r.dataset.i)].key] = id; }
        if (!Object.keys(mappings).length) return toast('还没有选择任何分类', true);
        const btn = res.querySelector('#gimp');
        btn.disabled = true;
        try {
          const rep = await api('/admin/github/import', { method: 'POST', body: { scan_id: scan.scan_id, depth: scan.depth, mappings } });
          res.querySelector('#gsum').textContent = `完成：新增 ${rep.created} 份资料（待核实），已存在跳过 ${rep.skipped_existing} 份，未选分类 ${rep.unmapped} 份`;
          toast('导入完成');
        } catch (err) { toast(err.message, true); }
        btn.disabled = false;
      };
    };
  },
  async status(box) {
    const [s, th] = await Promise.all([api('/admin/status'), api('/admin/thumbs').catch(() => null)]);
    const st = s.stats;
    box.innerHTML = `<section class="card"><h3>概况</h3><dl class="kv">
        <dt>已发布资料</dt><dd>${st.resources}</dd><dt>待审</dt><dd>${st.pending}</dd><dt>待处理投诉</dt><dd>${st.reports}</dd>
        <dt>用户</dt><dd>${st.users}</dd><dt>存储总量</dt><dd>${fmtSize(st.stored_bytes)}</dd><dt>下载次数</dt><dd>${st.downloads}</dd>
        <dt>今日中转</dt><dd>${s.relay_bytes_today != null ? fmtSize(s.relay_bytes_today) : '—'}</dd>
        <dt>邮件</dt><dd>${s.mail ? '已开启' : '<span class="badge rejected">未配置</span>'}</dd>
        <dt>内存占用</dt><dd>${s.rss_bytes ? fmtSize(s.rss_bytes) : '—'}</dd><dt>版本</dt><dd>${esc(s.version)}</dd></dl></section>
      ${th ? `<section class="card"><h3>列表缩略图</h3>
        <p class="small">已生成 <b>${th.done}</b> 个 · 待生成 <b>${th.todo}</b> 个 · 无法生成 ${th.never} 个（CAJ、程序、加密压缩包等）${th.current ? ` · 正在运行批次（${th.current.files} 个文件，${ago(th.current.dispatched_at)}开始）` : ''}</p>
        <p class="small muted">由 GitHub Actions 在 GitHub 内部渲染首页（PDF、Office、压缩包里的第一个文档、图片、文本），不经过服务器；每 10 分钟检查一次，每批最多 300 个。</p>
        ${me.level >= 4 ? '<button class="btn sm" id="thkick" type="button">立即检查</button>' : ''}</section>` : ''}
      <section class="card scroll-x"><h3>下载镜像</h3><p class="small muted">每 15 分钟从服务器经各镜像下载一个 256KB 探针文件，按实际速度排序；下载时依次尝试，全部失败时直连 GitHub。</p>
        ${s.mirrors.length ? `<table class="table"><thead><tr><th>镜像</th><th>状态</th><th>速度</th><th>检测时间</th></tr></thead><tbody>
        ${s.mirrors.map((m) => `<tr><td class="mono">${esc(m.prefix)}</td><td>${m.ok ? '<span class="badge published">可用</span>' : `<span class="badge rejected">不可用</span> <span class="small faint">${esc(m.error)}</span>`}</td>
          <td>${m.ok ? `${(m.speed_kbps / 1024).toFixed(2)} MB/s` : '—'}</td><td class="small faint">${ago(m.checked_at)}</td></tr>`).join('')}</tbody></table>` : '<p class="faint">尚未探测（本地存储模式或刚启动）</p>'}
      </section>`;
    const k = box.querySelector('#thkick');
    if (k) k.onclick = async () => { try { await api('/admin/thumbs', { method: 'POST' }); toast('已触发'); panels.status(box); } catch (e) { toast(e.message, true); } };
  },
};

async function show(name) {
  if (!panels[name]) name = 'dash';
  current = name;
  // Keep the tab in the address bar so a refresh (or a shared link) stays here.
  if (location.hash.slice(1) !== name) history.replaceState(null, '', `#${name}`);
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
  show(location.hash.slice(1) || 'dash');
  window.addEventListener('hashchange', () => { if (location.hash.slice(1) !== current) show(location.hash.slice(1)); });
})();
