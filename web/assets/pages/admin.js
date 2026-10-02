import { ago, api, avatar, esc, fmtDate, fmtSize, LEVELS, layout, loginUrl, meta, modal, moveResources, pathText, preview, resourceItem, store, toast, tree, $ } from '../app.js';

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
/** Questions to the uploader about one file and their answers (「问上传者」). */
function questionsHtml(r) {
  if (!r.questions || !r.questions.length) return '';
  return `<div class="small" style="padding:0 4px 10px 58px">${r.questions.map((q) => `<div class="notice${q.answer ? ' ok' : ''}" style="margin:4px 0">
    <b>${esc(q.asker)} 问：</b>${esc(q.text)}<br>${q.answer ? `<b>上传者答：</b>${esc(q.answer)}` : '<span class="faint">等待上传者回答</span>'}</div>`).join('')}</div>`;
}

/** A list of files to review. `opts.batch`: rows also get 「不懂」 (hand back to the pool);
 * `opts.onEmpty`: called when the last row is dealt with. */
async function queueUI(box, items, intro, actions, categoryFilters = false, opts = {}) {
  if (!items.length) {
    box.innerHTML = '<div class="card empty"><b>这里是空的</b>辛苦了 ☕</div>';
    return;
  }
  const [M, t] = await Promise.all([meta(), tree()]);
  const categories = categoryFilters ? t : null;
  // Where a file sits, as 「栏目 / 学院 / 课程」 linking to the course (same-named courses
  // exist in several colleges).
  const where = (r) => {
    const chain = [];
    for (let n = t.byId.get(r.node.id); n; n = t.byId.get(n.parent)) chain.unshift(n);
    return `<a class="faint" data-where href="/n/${r.node.id}" target="_blank" title="在新标签页打开这门课">${esc(chain.map((n) => n.name).join(' / ') || r.node.name)}</a>`;
  };
  const byId = new Map(items.map((r) => [String(r.id), r]));
  const locations = new Map();
  const groups = new Map();
  const courses = new Map();
  if (categories) {
    for (const r of items) {
      const chain = [];
      for (let n = categories.byId.get(r.node.id); n; n = categories.byId.get(n.parent)) chain.unshift(n);
      // The first child of a section is the offering group. Some older college nodes
      // have kind=course, so position is more reliable than kind for this level.
      const group = chain[1] || chain[0];
      const course = chain.slice(2).find((n) => n.kind === 'course');
      locations.set(String(r.id), { group: group?.id || 0, course: course?.id || 0 });
      if (group) groups.set(group.id, { node: group, section: chain[0] });
      if (course) courses.set(course.id, { node: course, group: group?.id || 0 });
    }
  }
  box.innerHTML = `<section class="card"><p class="small muted">${intro}</p>
    ${categories ? `<div class="bulkbar review-filters">
      <label class="small" for="qgroup">学院 / 分组</label><select class="input" id="qgroup"><option value="">全部学院 / 分组</option>${[...groups.values()].sort((a, b) => a.section.name.localeCompare(b.section.name, 'zh-CN') || a.node.name.localeCompare(b.node.name, 'zh-CN')).map(({ node, section }) => `<option value="${node.id}">${esc(section.name)} / ${esc(node.name)}</option>`).join('')}</select>
      <label class="small" for="qcourse">课程</label><select class="input" id="qcourse"><option value="">全部课程</option></select>
      <span class="small muted" id="qshown"></span></div>` : ''}
    <div class="bulkbar"><label class="small"><input type="checkbox" id="qall"> 全选</label>
      ${actions.map(([a, l, c]) => `<button class="btn sm ${c}" data-bulk="${a}">批量${l}</button>`).join('')}
      <button class="btn sm" data-move type="button">批量移动分类</button>
      <span class="grow"></span><span class="small muted">共 ${items.length} 份</span></div>
    <div class="list">${items.map((r) => `<div data-id="${r.id}">${resourceItem(r)}
      <div class="row small" style="padding:0 4px 14px 58px;gap:8px">
        <input type="checkbox" class="qsel">
        ${where(r)}<button class="btn sm" data-recourse type="button" title="在分类树里选另一门课，文件名随之更新">改课程</button>
        <span class="faint">${esc(r.original_name || '')}</span>
        ${r.uploader ? `<span class="faint">· ${esc(r.uploader.nickname)}</span>` : ''}<span class="faint">· ${ago(r.created_at)}</span>
        <span class="grow"></span>
        ${typeSelect(M, r)}
        <input class="input" data-f="note" placeholder="备注" style="max-width:180px;min-height:30px;padding:3px 8px">
        <button class="btn sm" data-pv type="button">预览</button>
        ${actions.map(([a, l, c]) => `<button class="btn sm ${c}" data-a="${a}">${l}</button>`).join('')}
        ${opts.batch ? '<button class="btn sm" data-skip type="button" title="拿不准，留给别的审核员">不懂</button>' : ''}
        <button class="btn sm" data-ask type="button" title="问上传者一个问题，回答前这份先不派给别人">问上传者</button>
        <a class="btn sm" href="/r/${r.id}" target="_blank">打开</a>
      </div>${questionsHtml(r)}</div>`).join('')}</div></section>`;
  const gone = (wrap) => {
    wrap.remove();
    updateVisible();
    if (!box.querySelector('.list > [data-id]') && opts.onEmpty) opts.onEmpty();
  };
  const act = async (wrap, action) => {
    await api(`/resources/${wrap.dataset.id}/review`, { method: 'POST', body: { action, note: wrap.querySelector('[data-f="note"]').value } });
    gone(wrap);
  };
  const visibleRows = () => [...box.querySelectorAll('.list > [data-id]')].filter((w) => !w.hidden);
  const updateVisible = () => {
    if (!categories) return;
    const group = box.querySelector('#qgroup').value;
    const course = box.querySelector('#qcourse').value;
    for (const wrap of box.querySelectorAll('.list > [data-id]')) {
      const place = locations.get(wrap.dataset.id);
      wrap.hidden = !!((group && String(place.group) !== group) || (course && String(place.course) !== course));
      if (wrap.hidden) wrap.querySelector('.qsel').checked = false;
    }
    const shown = visibleRows();
    box.querySelector('#qshown').textContent = `显示 ${shown.length} 份`;
    box.querySelector('#qall').checked = shown.length > 0 && shown.every((w) => w.querySelector('.qsel').checked);
  };
  if (categories) {
    const groupSelect = box.querySelector('#qgroup');
    const courseSelect = box.querySelector('#qcourse');
    const fillCourses = () => {
      const group = groupSelect.value;
      const available = [...courses.values()].filter((x) => !group || String(x.group) === group)
        .sort((a, b) => a.node.name.localeCompare(b.node.name, 'zh-CN'));
      courseSelect.innerHTML = `<option value="">全部课程</option>${available.map(({ node, group: id }) => `<option value="${node.id}">${group ? '' : `${esc(groups.get(id)?.node.name || '')} / `}${esc(node.name)}</option>`).join('')}`;
      courseSelect.value = '';
      updateVisible();
    };
    groupSelect.onchange = fillCourses;
    courseSelect.onchange = updateVisible;
    fillCourses();
  }
  box.onchange = async (e) => {
    if (e.target.matches('.qsel') && categories) {
      const shown = visibleRows();
      box.querySelector('#qall').checked = shown.length > 0 && shown.every((w) => w.querySelector('.qsel').checked);
      return;
    }
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
    const skip = e.target.closest('[data-skip]');
    if (skip) {
      const wrap = skip.closest('[data-id]');
      try { await api(`/review/batch/skip/${wrap.dataset.id}`, { method: 'POST' }); toast('已退回，留给别的审核员'); gone(wrap); } catch (err) { toast(err.message, true); }
      return;
    }
    const ask = e.target.closest('[data-ask]');
    if (ask) {
      const wrap = ask.closest('[data-id]');
      const text = prompt('想问上传者什么？对方会在资料页和「我的」页看到，回答后这份会回到待审池子。');
      if (!text || !text.trim()) return;
      try { await api(`/resources/${wrap.dataset.id}/questions`, { method: 'POST', body: { text } }); toast('已发给上传者'); gone(wrap); } catch (err) { toast(err.message, true); }
      return;
    }
    const rc = e.target.closest('[data-recourse]');
    if (rc) {
      const wrap = rc.closest('[data-id]');
      if (!(await moveResources([Number(wrap.dataset.id)]))) return;
      try {
        const r = await api(`/resources/${wrap.dataset.id}`);
        byId.set(String(r.id), r);
        const item = wrap.querySelector('.item');
        if (item) item.outerHTML = resourceItem(r);
        wrap.querySelector('[data-where]').outerHTML = where(r);
      } catch (err) { toast(err.message, true); }
      return;
    }
    if (e.target.closest('[data-move]')) {
      const picked = visibleRows().filter((w) => w.querySelector('.qsel').checked);
      if (await moveResources(picked.map((w) => Number(w.dataset.id)))) show(current);
      return;
    }
    const one = e.target.closest('[data-a]');
    const bulk = e.target.closest('[data-bulk]');
    try {
      if (one) { await act(one.closest('[data-id]'), one.dataset.a); toast('已处理'); }
      if (bulk) {
        const picked = visibleRows().filter((w) => w.querySelector('.qsel').checked);
        if (!picked.length) return toast('先勾选资料', true);
        bulk.disabled = true;
        try { for (const w of picked) await act(w, bulk.dataset.bulk); }
        finally { bulk.disabled = false; }
        toast(`已处理 ${picked.length} 份`);
      }
    } catch (err) { toast(err.message, true); }
  };
  box.querySelector('#qall').onchange = (e) => visibleRows().forEach((w) => { w.querySelector('.qsel').checked = e.target.checked; });
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

// 要求补证明: a takedown claim without proof is answered with what to send, and how (a new
// 申请下架 with an email); the admin can also write to the email left with it.
function proofLink(page) {
  return `${location.origin}/feedback?takedown=1${page && page.startsWith('/r/') ? `&from=${encodeURIComponent(page)}` : ''}`;
}
function proofText(page) {
  return `下架申请需要证明资料确实属于你：请写明资料链接和你的身份说明，附上证明材料（原始文件或手稿、编写 / 整理记录、署名或出版信息、教师或单位说明等），并留下邮箱，我们会通过邮箱联系你核实，图片和文件可以等邮件联系后再发。请在这里重新提交：${proofLink(page)}　没有证明材料或证明不足的申请一律驳回。`;
}
function proofMail(to, page) {
  const subject = '关于你在本站提交的资料下架申请';
  return `mailto:${encodeURIComponent(to)}?subject=${encodeURIComponent(subject)}&body=${encodeURIComponent(`你好，\n\n我们收到了你对 ${location.origin}${page || ''} 的下架申请。本站只在权利人证明资料确实属于本人后才下架，请直接回复本邮件，写明你的身份说明，并附上证明材料（原始文件或手稿、编写 / 整理记录、署名或出版信息、教师或单位说明等）。核实后我们会尽快处理；没有证明材料或证明不足的申请将被驳回。\n\n谢谢。`)}`;
}
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
    const intro = '「待审核」来自贡献者，通过后才会公开；「待复核」来自可信贡献者，已公开。';
    const actions = [['approve', '通过', 'ok'], ['reject', '驳回', 'danger']];
    // Admins can still see the whole queue at once; everyone works in batches by default.
    if (me.level >= 4 && store.get(ALL_KEY) === '1') {
      box.innerHTML = '<div class="row" style="margin-bottom:10px"><span class="small muted">正在查看全部待审资料（不分批）。</span><a href="#" id="tobatch" class="small">回到分批领取</a></div><div id="qbody"></div>';
      box.querySelector('#tobatch').onclick = (e) => { e.preventDefault(); store.set(ALL_KEY, null); panels.queue(box); };
      await queueUI(box.querySelector('#qbody'), await api('/review'), intro, actions, true);
      return;
    }
    const t = await tree();
    const within = store.get(WITHIN_KEY) || '';
    const opts = t.children(0).map((sec) => `<option value="${sec.id}">${esc(sec.name)}（全部）</option>${t.children(sec.id).filter((g) => t.children(g.id).length).map((g) => `<option value="${g.id}">　${esc(sec.name)} / ${esc(g.name)}</option>`).join('')}`).join('');
    const load = async (take) => {
      const q = within ? `?within=${within}` : '';
      const data = take ? await api('/review/batch', { method: 'POST', body: { within: within ? Number(within) : null } }) : await api(`/review/batch${q}`);
      box.innerHTML = `<section class="card"><div class="row" style="gap:10px;flex-wrap:wrap">
          <label class="small" for="bwithin">只审</label><select class="input" id="bwithin" style="max-width:320px"><option value="">全部学院 / 分组</option>${opts}</select>
          <button class="btn sm primary" id="btake" ${data.items.length ? 'disabled title="这一批审完才能领下一批"' : ''}>领取一批（最多 ${data.size} 份）</button>
          <span class="small muted">池子里还有 ${data.pool} 份待领取</span><span class="grow"></span>
          ${me.level >= 4 ? '<a href="#" id="toall" class="small">查看全部待审</a>' : ''}</div>
        <p class="small muted" style="margin:8px 0 0">每人每次随机领一批、互不重复，审完才能领下一批。离开这个页面几分钟后，没审完的会自动回到池子里。拿不准的点「不懂」留给别人；需要问上传者的点「问上传者」。</p></section>
        <div id="qbody" style="margin-top:14px"></div>`;
      box.querySelector('#bwithin').value = within;
      box.querySelector('#bwithin').onchange = async (e) => {
        store.set(WITHIN_KEY, e.target.value || null);
        // The choice applies to the next batch; the one in hand stays until it is done.
        panels.queue(box);
      };
      box.querySelector('#btake').onclick = () => load(true).catch((err) => toast(err.message, true));
      const all = box.querySelector('#toall');
      if (all) all.onclick = async (e) => { e.preventDefault(); await releaseBatch(); store.set(ALL_KEY, '1'); panels.queue(box); };
      const body = box.querySelector('#qbody');
      if (!data.items.length) {
        body.innerHTML = `<div class="card empty"><b>${take ? '池子里没有可领的了' : '手上没有待审的'}</b>${data.pool ? '点上面的「领取一批」开始' : '辛苦了 ☕'}</div>`;
        return;
      }
      await queueUI(body, data.items, intro, actions, true, {
        batch: true,
        onEmpty: () => { toast('这一批审完了，可以领下一批'); load(false); },
      });
    };
    await load(false);
    heartbeat(() => api(`/review/batch${within ? `?within=${within}` : ''}`).catch(() => {}));
  },
  async changes(box) {
    const list = await api('/review/change-requests');
    box.innerHTML = `<section class="card"><p class="small muted">上传者申请修改公开备注或删除自己的资料；同意后才会生效。删除获批后资料下架。</p>
      ${list.length ? list.map(({ request: q, title, current_note, uploader }) => `<div class="item" data-id="${q.id}"><div class="body">
        <div><a class="title" href="/r/${q.resource}" target="_blank">${esc(title)}</a></div>
        <div class="meta"><span>${q.kind === 'note' ? '修改备注' : '删除资料'}</span><span>上传者：${esc(uploader)}</span><span>${ago(q.created_at)}</span></div>
        ${q.kind === 'note' ? `<p class="small">原备注：${esc(current_note || '（无）')}</p><p class="small">申请改为：${esc(q.value || '（清空）')}</p>` : `<p class="small">删除理由：${esc(q.value)}</p>`}
        <div class="row"><input class="input" data-review-note placeholder="审核意见（驳回时建议填写）" maxlength="300" style="max-width:320px">
          <button class="btn sm ok" data-decision="approve" type="button">同意</button><button class="btn sm danger" data-decision="reject" type="button">驳回</button></div>
      </div></div>`).join('') : '<div class="empty"><b>没有待处理的申请</b></div>'}</section>`;
    box.onclick = async (e) => {
      const button = e.target.closest('[data-decision]');
      if (!button) return;
      const row = button.closest('[data-id]');
      button.disabled = true;
      try {
        await api(`/review/change-requests/${row.dataset.id}`, { method: 'POST', body: {
          approve: button.dataset.decision === 'approve', note: row.querySelector('[data-review-note]').value,
        } });
        row.remove();
        toast('申请已处理');
        if (!box.querySelector('[data-id]')) box.querySelector('section').insertAdjacentHTML('beforeend', '<div class="empty"><b>没有待处理的申请</b></div>');
      } catch (err) { button.disabled = false; toast(err.message, true); }
    };
  },
  async uncertain(box) {
    await queueUI(box, await api('/review?status=pending&uncertain=true'), '同学放进「待整理」的资料（不知道该放哪门课，先看备注），以及导入时标注「不确定」的资料。先「批量移动分类」或打开资料改好分类和名称，再通过；还在「待整理」里的不能直接通过。',
      [['approve', '确认并公开', 'ok'], ['restrict', '设为仅内部', ''], ['reject', '驳回', 'danger']]);
  },
  async avatars(box) {
    const list = await api('/admin/avatars');
    if (!list.length) { box.innerHTML = '<div class="card empty"><b>没有待审核的头像</b></div>'; return; }
    box.innerHTML = `<section class="card"><p class="small muted">头像对所有人公开：色情、暴力、广告、冒充他人或学校官方的一律驳回。驳回后对方原来的头像不变。</p>
      <div class="list">${list.map((a) => `<div class="item" data-user="${a.user}">
        ${avatar(a.pending, a.nickname, 72)}
        <div class="body"><b>${esc(a.nickname)}</b><div class="meta"><span>${ago(a.at)}</span>${a.current.length ? `<span>原头像</span>${avatar(a.current, a.nickname, 28)}` : '<span>原来没有头像</span>'}</div></div>
        <div class="row"><button class="btn sm ok" data-a="approve">通过</button><button class="btn sm danger" data-a="reject">驳回</button></div>
      </div>`).join('')}</div></section>`;
    box.onclick = async (e) => {
      const b = e.target.closest('[data-a]');
      if (!b) return;
      const row = b.closest('[data-user]');
      try {
        await api(`/admin/avatars/${row.dataset.user}`, { method: 'POST', body: { action: b.dataset.a } });
        row.remove();
        if (!box.querySelector('[data-user]')) panels.avatars(box);
      } catch (err) { toast(err.message, true); }
    };
  },
  async restricted(box) {
    await queueUI(box, await api('/review?status=restricted'), '「仅内部」资料不公开、不可搜索（勿外传、加密题库等）。确认获得授权后可以恢复发布。',
      [['restore', '恢复发布', 'ok'], ['reject', '驳回', 'danger']]);
  },
  async reports(box, all = false) {
    const list = await api(`/admin/reports${all ? '?all=true' : ''}`);
    const STATUS = { pending: '待审核', published: '已发布', rejected: '未通过', removed: '已下架', restricted: '仅内部' };
    box.innerHTML = `<section class="card"><div class="row" style="margin-bottom:8px"><p class="small muted" style="margin:0">请在 48 小时内处理。以权利人身份申请下架的，<b>必须有足以证明资料属于对方的材料</b>才下架（先点「下架」再标记已处理）；没有证明或证明不足的，点「发邮件要证明」联系投诉人，再点「要求补证明」并标记已处理（暂不下架）。</p><span class="grow"></span>
        <label class="small"><input type="checkbox" id="rpall"${all ? ' checked' : ''}> 显示已处理</label></div>
      ${list.length ? list.map((r) => `
      <div class="item" data-id="${r.id}" data-res="${r.resource ? r.resource.id : ''}"><div class="body">
        <div><b>${esc(r.reason)}</b></div>
        <div class="meta"><span>${ago(r.created_at)}</span>${r.contact ? `<span>投诉人：${esc(r.contact)}</span><a href="${esc(proofMail(r.contact, r.resource ? `/r/${r.resource.id}` : ''))}">发邮件要证明</a>` : ''}
          ${r.resource ? `<a href="/r/${r.resource.id}" target="_blank">${esc(r.resource.title)}</a><span class="badge ${esc(r.resource.status)}">${STATUS[r.resource.status] || esc(r.resource.status)}</span>` : '<span>资料已不存在</span>'}
          ${r.handled ? `<span class="badge published">已处理${r.handled_by ? ` · ${esc(r.handled_by)}` : ''}</span>${r.handled_note ? `<span>${esc(r.handled_note)}</span>` : ''}` : ''}
          ${me.level >= 4 ? '<a href="#" class="small" data-a="delete" style="color:var(--bad)">删除</a>' : ''}</div>
        ${r.handled ? '' : `<div class="row" style="margin-top:8px"><input class="input" placeholder="处理说明" style="max-width:260px;min-height:30px;padding:3px 8px">
          ${r.resource && r.resource.status !== 'removed' ? '<button class="btn sm danger" data-a="remove">下架</button>' : ''}<button class="btn sm" data-a="proof" title="填入「已邮件要求补充证明材料，暂不下架」">要求补证明</button><button class="btn sm ok" data-a="done">标记已处理</button></div>`}
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
        if (b.dataset.a === 'proof') { it.querySelector('input').value = '未附权属证明，已邮件要求投诉人补充证明材料，暂不下架；补充后可重新投诉'; return; }
        if (b.dataset.a === 'remove') { await api(`/resources/${it.dataset.res}/review`, { method: 'POST', body: { action: 'remove', note: note || '收到投诉，已下架' } }); toast('已下架'); panels.reports(box, all); }
        else { await api(`/admin/reports/${it.dataset.id}/handle`, { method: 'POST', body: { note } }); toast('已处理，可在「显示已处理」中查看'); panels.reports(box, all); }
      } catch (err) { toast(err.message, true); }
    };
  },
  async feedback(box, all = false) {
    const list = await api(`/admin/feedback${all ? '?all=true' : ''}`);
    box.innerHTML = `<section class="card"><div class="row" style="margin-bottom:8px"><p class="small muted" style="margin:0">用户从「意见反馈」页提交的问题和建议。标着【申请下架】的：有足以证明资料属于对方的材料才下架；没有的点「要求补证明」回复（对方在反馈页能看到），留了邮箱的也可以点「发邮件要证明」直接联系。</p><span class="grow"></span>
        <label class="small"><input type="checkbox" id="fball"${all ? ' checked' : ''}> 显示已处理</label></div>
      ${list.length ? list.map((f) => `<div class="item" data-id="${f.id}"><div class="body">
        <div style="white-space:pre-wrap;overflow-wrap:anywhere">${esc(f.body)}</div>
        <div class="meta"><span>${ago(f.created_at)}</span>${f.nickname ? `<span>${esc(f.nickname)}</span>` : '<span>未登录</span>'}${f.contact ? `<span>联系：${esc(f.contact)}</span>${f.contact.includes('@') ? `<a href="${esc(proofMail(f.contact, f.page))}">发邮件要证明</a>` : ''}` : ''}${f.page ? `<span class="faint">${esc(f.page)}</span>` : ''}
          ${f.handled ? `<span class="badge published">已处理${f.handled_by ? ` · ${esc(f.handled_by)}` : ''}</span>${f.handled_note ? `<span>回复：${esc(f.handled_note)}</span>` : ''}` : ''}</div>
        ${f.handled ? '' : `<div class="row" style="margin-top:8px"><input class="input" placeholder="回复反馈人（选填，对方在反馈页能看到）" maxlength="500" style="max-width:360px;min-height:30px;padding:3px 8px"><button class="btn sm" data-a="proof" title="下架申请没有证明材料时用：填入请对方补充证明并留邮箱的回复">要求补证明</button><button class="btn sm ok" data-a="done">回复并标记已处理</button></div>`}
      </div></div>`).join('') : '<div class="empty"><b>没有待处理的反馈</b></div>'}</section>`;
    box.querySelector('#fball').onchange = (e) => panels.feedback(box, e.target.checked);
    box.onclick = async (e) => {
      const b = e.target.closest('[data-a]');
      if (!b) return;
      const it = b.closest('[data-id]');
      if (b.dataset.a === 'proof') { it.querySelector('input').value = proofText(list.find((f) => String(f.id) === it.dataset.id)?.page || ''); return; }
      if (b.dataset.a !== 'done') return;
      try { await api(`/admin/feedback/${it.dataset.id}/handle`, { method: 'POST', body: { note: it.querySelector('input').value } }); toast('已处理'); panels.feedback(box, all); } catch (err) { toast(err.message, true); }
    };
  },
  async links(box) {
    const list = await api('/links/suggestions');
    const host = (u) => { try { return new URL(u).host; } catch { return ''; } };
    box.innerHTML = `<section class="card"><p class="small muted" style="margin-top:0">同学推荐的站外资源，采纳后显示在<a href="/links" target="_blank">站外资源</a>页。先打开链接看一下：是学习资料、不是钓鱼或广告、不需要付费再采纳；名称和说明可以改好再采纳。不采纳要写原因（推荐人能看到）。自己推荐的要由其他审核员审核。</p>
      ${list.length ? list.map((s) => `<div class="item" data-id="${s.id}"><div class="body">
        <div class="fields-2"><label class="field"><span>名称</span><input class="input" data-f="title" maxlength="40" value="${esc(s.title)}"></label>
          <label class="field"><span>链接 · <a href="${esc(s.url)}" target="_blank" rel="noopener noreferrer nofollow">打开看看</a> <span class="faint mono">${esc(host(s.url))}</span></span><input class="input" data-f="url" maxlength="400" value="${esc(s.url)}"></label></div>
        <label class="field"><span>说明</span><input class="input" data-f="note" maxlength="200" value="${esc(s.note)}"></label>
        <div class="meta"><span>${avatar(s.by.avatar, s.by.nickname, 20)} ${esc(s.by.nickname)} 推荐</span><span>${ago(s.created_at)}</span></div>
        ${s.mine && me.level < 4 ? '<p class="small faint">这是你推荐的，要由其他审核员审核。</p>' : `<div class="row" style="margin-top:8px"><label class="small">排序 <input class="input" data-f="sort" type="number" min="0" value="100" style="max-width:80px;min-height:30px;padding:3px 8px"></label><button class="btn sm ok" data-a="ok" type="button">采纳</button>
          <input class="input" data-f="reason" placeholder="不采纳的原因（推荐人能看到）" maxlength="200" style="max-width:300px;min-height:30px;padding:3px 8px"><button class="btn sm danger" data-a="no" type="button">不采纳</button></div>`}
      </div></div>`).join('') : '<div class="empty"><b>没有待审核的推荐</b></div>'}</section>`;
    box.onclick = async (e) => {
      const b = e.target.closest('[data-a]');
      if (!b) return;
      const it = b.closest('[data-id]');
      const f = (k) => it.querySelector(`[data-f="${k}"]`).value;
      const approve = b.dataset.a === 'ok';
      if (!approve && !f('reason').trim()) return toast('写一下不采纳的原因', true);
      const body = approve ? { approve, title: f('title'), url: f('url'), note: f('note'), sort: Number(f('sort')) || 0 } : { approve, reason: f('reason') };
      try { await api(`/links/suggestions/${it.dataset.id}/review`, { method: 'POST', body }); it.remove(); toast(approve ? '已加入站外资源' : '已处理'); } catch (err) { toast(err.message, true); }
    };
  },
  async moves(box) {
    const list = await api('/move-suggestions');
    box.innerHTML = `<section class="card"><p class="small muted" style="margin-top:0">同学认为资料放错了课程，建议移到别处。打开资料看一下，对的就「采纳并移动」（文件名会跟着新课程改），不对就写原因不采纳；提建议的人会在站内提醒里看到结果。</p>
      ${list.length ? list.map((s) => `<div class="item" data-id="${s.id}"><div class="body">
        <a class="title" href="/r/${s.resource}" target="_blank">${esc(s.title || `#${s.resource}`)}</a>
        <div class="small" style="margin:4px 0"><a href="/n/${s.from_id}" target="_blank">${esc(s.from)}</a> → <b><a href="/n/${s.to_id}" target="_blank">${esc(s.to)}</a></b></div>
        ${s.note ? `<div class="note">${esc(s.note)}</div>` : ''}
        <div class="meta"><span>${avatar(s.by.avatar, s.by.nickname, 20)} ${esc(s.by.nickname)} 建议</span><span>${ago(s.created_at)}</span></div>
        ${s.mine ? '<p class="small faint">这是你提的建议，要由其他审核员处理。</p>' : `<div class="row" style="margin-top:8px"><button class="btn sm ok" data-a="ok" type="button">采纳并移动</button>
          <input class="input" placeholder="不采纳的原因（提建议的人能看到）" maxlength="200" style="max-width:300px;min-height:30px;padding:3px 8px"><button class="btn sm danger" data-a="no" type="button">不采纳</button></div>`}
      </div></div>`).join('') : '<div class="empty"><b>没有待处理的分类建议</b></div>'}</section>`;
    box.onclick = async (e) => {
      const b = e.target.closest('[data-a]');
      if (!b) return;
      const it = b.closest('[data-id]');
      const approve = b.dataset.a === 'ok';
      const reason = it.querySelector('input').value.trim();
      if (!approve && !reason) return toast('写一下不采纳的原因', true);
      try { await api(`/move-suggestions/${it.dataset.id}/review`, { method: 'POST', body: { approve, reason } }); it.remove(); toast(approve ? '已移动' : '已处理'); } catch (err) { toast(err.message, true); }
    };
  },
  async missing(box, show = 'new') {
    const all = await api('/admin/missing');
    const S = { new: '待处理', public: '已公开征集', hidden: '已隐藏' };
    const list = all.filter((m) => m.status === show);
    box.innerHTML = `<section class="card"><p class="small muted" style="margin-top:0">同学搜了但一条结果都没有的词（每人每天同一个词只算一次）。是本站缺的课程就「公开征集」，会列在<a href="/missing" target="_blank">缺资料的课程</a>页，有资料后自动从那里消失；乱码、小说游戏、不适合公开的就「隐藏」。标了「现在能搜到」的说明已经有资料或加了别名。</p>
      <div class="tabs" style="margin-bottom:10px">${Object.entries(S).map(([k, v]) => `<button type="button" data-s="${k}" class="${k === show ? 'on' : ''}">${v} ${all.filter((m) => m.status === k).length}</button>`).join('')}</div>
      ${list.length ? `<table class="table"><thead><tr><th>搜索词</th><th>人次</th><th>最近</th><th></th></tr></thead><tbody>
      ${list.map((m) => `<tr data-term="${esc(m.term)}"><td><a href="/search?q=${encodeURIComponent(m.term)}" target="_blank">${esc(m.term)}</a>${m.found ? ' <span class="badge published">现在能搜到</span>' : ''}</td><td>${m.count}</td><td class="small faint">${ago(m.last)}</td>
        <td class="row" style="gap:6px;justify-content:flex-end">${show !== 'public' ? '<button class="btn sm ok" data-to="public" type="button">公开征集</button>' : ''}${show !== 'hidden' ? '<button class="btn sm" data-to="hidden" type="button">隐藏</button>' : ''}${show !== 'new' ? '<button class="btn sm" data-to="new" type="button">撤回</button>' : ''}</td></tr>`).join('')}</tbody></table>`
      : '<div class="empty"><b>这里没有</b></div>'}</section>`;
    box.onclick = async (e) => {
      const tab = e.target.closest('[data-s]');
      if (tab) { panels.missing(box, tab.dataset.s); return; }
      const b = e.target.closest('[data-to]');
      if (!b) return;
      const tr = b.closest('[data-term]');
      try { await api('/admin/missing/status', { method: 'POST', body: { term: tr.dataset.term, status: b.dataset.to } }); tr.remove(); toast('已更新'); } catch (err) { toast(err.message, true); }
    };
  },
  async collections(box) {
    const list = await api('/review/collections');
    box.innerHTML = `<section class="card"><p class="small muted" style="margin-top:0">同学申请公开分享的收藏夹，通过后显示在<a href="/collections" target="_blank">收藏夹</a>页。看名字和说明是否合适（没有广告、联系方式、不当内容），点开看看里面的资料。不通过要写原因（本人能看到）。自己的收藏夹要由其他审核员审核。</p>
      ${list.length ? list.map((c) => `<div class="item" data-id="${c.id}"><div class="body">
        <a class="title" href="/collections?id=${c.id}" target="_blank">${esc(c.title)}</a>${c.note ? `<div class="note">${esc(c.note)}</div>` : ''}
        <div class="meta"><span>${c.count} 份</span><span>${avatar(c.owner.avatar, c.owner.nickname, 20)} ${esc(c.owner.nickname)}</span><span>${ago(c.updated_at)}</span></div>
        ${c.mine && me.level < 4 ? '<p class="small faint">这是你的收藏夹，要由其他审核员审核。</p>' : `<div class="row" style="margin-top:8px"><button class="btn sm ok" data-a="ok" type="button">通过</button>
          <input class="input" placeholder="不通过的原因（本人能看到）" maxlength="200" style="max-width:300px;min-height:30px;padding:3px 8px"><button class="btn sm danger" data-a="no" type="button">不通过</button></div>`}
      </div></div>`).join('') : '<div class="empty"><b>没有待审核的收藏夹</b></div>'}</section>`;
    box.onclick = async (e) => {
      const b = e.target.closest('[data-a]');
      if (!b) return;
      const it = b.closest('[data-id]');
      const note = it.querySelector('input').value.trim();
      if (b.dataset.a === 'no' && !note) return toast('写一下不通过的原因', true);
      try { await api(`/collections/${it.dataset.id}/review`, { method: 'POST', body: { approve: b.dataset.a === 'ok', note } }); it.remove(); toast(b.dataset.a === 'ok' ? '已公开' : '已处理'); } catch (err) { toast(err.message, true); }
    };
  },
  async series(box) {
    const list = await api('/review/series');
    const itemLi = (i, cls) => `<li class="${cls}"><a href="/r/${i.id}" target="_blank">${esc(i.title)}</a>${i.status !== 'published' ? ` <span class="badge pending">${i.status === 'pending' ? '待审' : esc(i.status)}</span>` : ''}</li>`;
    box.innerHTML = `<section class="card"><p class="small muted" style="margin-top:0">同学提议的合集：同一门课里按顺序排好的一组资料，通过后在课程页合成一行、资料页显示上一份 / 下一份。看名称是否合适、资料是否确实是一套、顺序对不对。
      <b>＋</b> 新加入的，<s>删除线</s> 是要移出的。不通过要写原因（提议人能看到）。审核员自己提议的要由其他审核员审核。</p>
      ${list.length ? list.map((s) => {
        const d = s.draft;
        const was = new Set(s.items.map((i) => i.id));
        const now = new Set(d.items.map((i) => i.id));
        const closing = !d.items.length;
        const renamed = s.status === 'public' && s.title !== d.title;
        return `<div class="item" data-id="${s.id}"><div class="body">
        <b>${closing ? `申请解散合集「${esc(s.title)}」` : esc(d.title)}</b>${renamed ? ` <span class="small faint">原名「${esc(s.title)}」</span>` : ''}
        ${!closing && (d.source || d.year) ? `<div class="small muted">${[d.source && `来源：${esc(d.source)}`, d.year && `年份：${esc(d.year)}`].filter(Boolean).join(' · ')}</div>` : ''}
        <div class="meta"><span>${s.status === 'public' ? '修改' : '新合集'}</span><a href="/n/${s.node}" target="_blank">${esc(s.node_name)}</a>
          <span>${avatar(d.by.avatar, d.by.nickname, 20)} ${esc(d.by.nickname)}${d.by.role ? ` · ${d.by.role}` : ''}</span><span>${ago(d.at)}</span></div>
        ${closing ? '' : `<ol class="series-review">${d.items.map((i) => itemLi(i, s.status === 'public' && !was.has(i.id) ? 'add' : '')).join('')}</ol>`}
        ${s.items.some((i) => !now.has(i.id)) ? `<ul class="series-review">${s.items.filter((i) => !now.has(i.id)).map((i) => itemLi(i, 'gone')).join('')}</ul>` : ''}
        ${d.mine && me.level < 4 ? '<p class="small faint">这是你提议的，要由其他审核员审核。</p>' : `<div class="row" style="margin-top:8px"><button class="btn sm ok" data-a="ok" type="button">通过</button>
          ${d.items.some((i) => i.status === 'pending') ? `<button class="btn sm ok" data-a="all" type="button" title="上传者已经把这些资料整理成一套，逐个点开确认过内容后可以一起通过">通过，并一起通过里面 ${d.items.filter((i) => i.status === 'pending').length} 份待审资料</button>` : ''}
          <input class="input" placeholder="不通过的原因（提议人能看到）" maxlength="200" style="max-width:300px;min-height:30px;padding:3px 8px"><button class="btn sm danger" data-a="no" type="button">不通过</button></div>`}
      </div></div>`;
      }).join('') : '<div class="empty"><b>没有待审核的合集</b></div>'}</section>`;
    box.onclick = async (e) => {
      const b = e.target.closest('[data-a]');
      if (!b) return;
      const it = b.closest('[data-id]');
      const note = it.querySelector('input').value.trim();
      if (b.dataset.a === 'no' && !note) return toast('写一下不通过的原因', true);
      const approve = b.dataset.a !== 'no';
      try {
        const r = await api(`/series/${it.dataset.id}/review`, { method: 'POST', body: { approve, note, with_files: b.dataset.a === 'all' } });
        it.remove();
        toast(!approve ? '已处理' : b.dataset.a === 'all' ? `已通过合集和其中 ${r.files_approved} 份资料` : '已通过');
      } catch (err) { toast(err.message, true); }
    };
  },
  async announce(box) {
    const a = (await meta()).announcement || { text: '' };
    box.innerHTML = `<section class="card"><h3 style="margin-top:0">顶部横幅公告</h3>
      <p class="small muted">显示在每个页面顶部；同学点 × 后不再显示，直到公告改了。留空则不显示。可以写网址，会自动变成链接。</p>
      <textarea class="input" id="antext" rows="3" maxlength="300">${esc(a.text)}</textarea>
      <div class="row" style="margin-top:8px"><button class="btn primary sm" id="ansave" type="button">保存</button><span class="small faint">保存后一分钟内所有人可见。</span></div></section>`;
    box.querySelector('#ansave').onclick = async () => {
      try { await api('/announcement', { method: 'PUT', body: { text: box.querySelector('#antext').value } }); toast('已保存'); } catch (err) { toast(err.message, true); }
    };
    const sec = document.createElement('section');
    sec.className = 'card';
    box.append(sec);
    const draw = async () => {
      const list = await api('/bulletins');
      sec.innerHTML = `<h3 style="margin-top:0">站内公告</h3>
        <p class="small muted">放在「站内提醒」页顶部，不弹出；发布时每个账号会收到一条未读提醒（页头「提醒」上的红点）。可以换行，网址会自动变成链接。</p>
        <textarea class="input" id="btext" rows="4" maxlength="1000" placeholder="公告内容"></textarea>
        <div class="row" style="margin-top:8px"><button class="btn primary sm" id="bpost" type="button">发布</button></div>
        <div class="list" style="margin-top:12px">${list.map((b) => `<div class="item" data-id="${b.id}"><div class="body"><div style="white-space:pre-wrap;overflow-wrap:anywhere">${esc(b.text)}</div>
          <div class="meta"><span class="faint">${fmtDate(b.at, true)}</span></div></div><button class="btn sm danger" data-del type="button">删除</button></div>`).join('') || '<p class="small faint">还没有站内公告。</p>'}</div>`;
      sec.querySelector('#bpost').onclick = async () => {
        const text = sec.querySelector('#btext').value.trim();
        if (!text) return toast('请填写公告内容', true);
        try { await api('/bulletins', { method: 'POST', body: { text } }); toast('已发布'); draw(); } catch (err) { toast(err.message, true); }
      };
      sec.querySelector('.list').onclick = async (e) => {
        const b = e.target.closest('[data-del]');
        if (!b) return;
        try { await api(`/bulletins/${b.closest('[data-id]').dataset.id}`, { method: 'DELETE' }); toast('已删除'); draw(); } catch (err) { toast(err.message, true); }
      };
    };
    draw().catch((err) => toast(err.message, true));
  },
  async log(box) {
    const list = await api('/admin/reviews');
    const A = { edit: ['上传者修改', 'pending'], approve: ['通过', 'published'], reject: ['驳回', 'rejected'], remove: ['下架', 'removed'], restrict: ['仅内部', 'restricted'], restore: ['恢复发布', 'published'], note_approved: ['同意修改备注', 'published'], note_rejected: ['驳回备注申请', 'rejected'], delete_approved: ['同意删除', 'removed'], delete_rejected: ['驳回删除申请', 'rejected'] };
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
      ${list.map((u) => `<tr data-id="${u.id}"><td>${esc(u.nickname)}</td><td class="small">${esc(u.email)}${u.xmu ? ' <span class="badge published">{{site.verified_label}}</span>' : ''}</td>
        <td>${u.id === me.id || u.level >= me.level ? `${LEVELS[u.level]}${u.level === 4 ? ' <span class="small faint">（名单）</span>' : ''}` : `<select class="input" data-f="level" style="min-height:30px;padding:2px 8px">${opts(u.level)}</select>`}</td>
        <td>${u.uploads}</td><td class="small faint">${fmtDate(u.created_at)}</td>
        <td>${u.id === me.id ? '<span class="small faint">你自己</span>' : u.level >= me.level ? '<span class="small faint">同级</span>' : `<button class="btn sm ${u.banned ? '' : 'danger'}" data-a="ban">${u.banned ? '解除封禁' : '封禁'}</button>${me.level >= 4 ? ` <button class="btn sm danger" data-a="purge" title="封禁账号，并撤掉他发的所有东西">封禁并清理</button>` : ''}`}</td></tr>`).join('')}
      </tbody></table></section>`;
    box.querySelector('#uq').onsubmit = (e) => { e.preventDefault(); panels.users(box, box.querySelector('#uqv').value); };
    box.onchange = async (e) => {
      const sel = e.target.closest('[data-f="level"]');
      if (!sel) return;
      try { await api(`/admin/users/${sel.closest('tr').dataset.id}`, { method: 'PATCH', body: { level: Number(sel.value) } }); toast('角色已更新'); } catch (err) { toast(err.message, true); }
    };
    box.onclick = async (e) => {
      const p = e.target.closest('[data-a="purge"]');
      if (p) return purgeDialog(list.find((u) => String(u.id) === p.closest('tr').dataset.id), () => panels.users(box, q));
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
        <div class="row" style="margin-top:12px"><input class="input" id="gmajor" maxlength="20" placeholder="适用专业（选填）" style="max-width:180px" title="整个仓库只属于某个专业时填，如某个软件工程专业的仓库填 软件工程"><button class="btn primary" id="gimp" type="button">导入已选择的组</button><span class="small muted" id="gsum"></span></div>`;
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
          const rep = await api('/admin/github/import', { method: 'POST', body: { scan_id: scan.scan_id, depth: scan.depth, mappings, major: res.querySelector('#gmajor').value.trim() } });
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

const ALL_KEY = 'xmuhub.review.all';
const WITHIN_KEY = 'xmuhub.review.within';

// While the 待审资料 tab is open it pings the server every minute so its batch stays held;
// leaving the tab or the page hands the batch back at once.
let beat = 0;
function heartbeat(fn) {
  clearInterval(beat);
  beat = setInterval(() => { if (document.visibilityState === 'visible') fn(); }, 60_000);
}
function releaseBatch() {
  clearInterval(beat);
  beat = 0;
  // keepalive: still sent while the page is being closed.
  return fetch('/api/review/batch/release', { method: 'POST', keepalive: true, headers: { 'X-XMUHub': '1' } }).catch(() => {});
}
window.addEventListener('pagehide', () => { if (current === 'queue') releaseBatch(); });

// Complaints, feedback, the full review log and accounts are admin-only (the server enforces it).
/** 封禁并清理: bans the account and takes back everything it posted, after one confirmation. */
function purgeDialog(u, done) {
  const body = modal(`封禁并清理「${u.nickname}」`);
  body.innerHTML = `<p class="small">会一次做完下面这些，<b>资料文件本身不删</b>（只改状态，需要时可以在资料页逐个恢复）：</p>
    <ul class="small muted" style="margin:0 0 10px;padding-left:20px">
      <li>封禁账号，退出所有登录，作废他的 API 令牌</li>
      <li>他上传的 ${u.uploads} 份资料：待审的标为未通过，已公开的下架</li>
      <li>他发过的评论和回复删除；他打的评分不再计入</li>
      <li>合集提议、推荐的链接驳回；收藏夹取消公开；头像清掉</li></ul>
    <label class="field"><span>原因（资料和帖子上会显示给审核员和本人）</span><input class="input" id="pg_reason" maxlength="200" value="账号因滥用被封禁，内容已清理"></label>
    <label class="field"><span>确认：输入昵称「${esc(u.nickname)}」</span><input class="input" id="pg_confirm" autocomplete="off"></label>
    <div class="row"><button class="btn danger" id="pg_go" type="button">封禁并清理</button><span class="small faint" id="pg_msg"></span></div>`;
  body.querySelector('#pg_go').onclick = async () => {
    if (body.querySelector('#pg_confirm').value.trim() !== u.nickname) return toast('昵称不对，没有执行', true);
    const go = body.querySelector('#pg_go');
    go.disabled = true;
    body.querySelector('#pg_msg').textContent = '处理中…';
    try {
      const r = await api(`/admin/users/${u.id}/purge`, { method: 'POST', body: { reason: body.querySelector('#pg_reason').value } });
      body.innerHTML = `<div class="notice ok">已封禁「${esc(u.nickname)}」并清理：资料未通过 ${r.rejected} 份、下架 ${r.removed} 份，评论 ${r.comments} 条，评分 ${r.ratings} 个，求资料 ${r.wants} 条、回复 ${r.want_replies} 条，合集提议 ${r.series} 个，收藏夹 ${r.collections} 个，推荐链接 ${r.links} 个，令牌 ${r.tokens} 个${r.avatar ? '，头像已清掉' : ''}。</div>`;
      done();
    } catch (err) { go.disabled = false; body.querySelector('#pg_msg').textContent = ''; toast(err.message, true); }
  };
}

const ADMIN_TABS = ['reports', 'feedback', 'log', 'users', 'announce', 'missing'];

async function show(name) {
  if (!panels[name] || (ADMIN_TABS.includes(name) && me.level < 4)) name = 'dash';
  if (current === 'queue' && name !== 'queue') releaseBatch();
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
  tabCounts();
}

/** How much waits in each queue, as a number on its tab (refreshed on every switch and each minute). */
async function tabCounts() {
  let c;
  try { c = await api('/review/counts'); } catch { return; }
  document.querySelectorAll('#tabs button').forEach((b) => {
    let s = b.querySelector('.count');
    const n = c[b.dataset.t] || 0;
    if (!s) { s = document.createElement('span'); s.className = 'count'; b.append(s); }
    s.textContent = n > 99 ? '99+' : String(n);
    s.hidden = !n;
  });
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
  if (me.level < 4) document.querySelectorAll('#tabs button').forEach((b) => { b.hidden = ADMIN_TABS.includes(b.dataset.t); });
  $('#tabs').onclick = (e) => { const b = e.target.closest('button'); if (b) show(b.dataset.t); };
  show(location.hash.slice(1) || 'dash');
  setInterval(() => { if (!document.hidden) tabCounts(); }, 60000);
  window.addEventListener('hashchange', () => { if (location.hash.slice(1) !== current) show(location.hash.slice(1)); });
})();
