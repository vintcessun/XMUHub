import { api, downloadResource, esc, fmtSize, layout, levelBadge, modal, moveResources, nodeCard, nodeTitle, pathId, resourceItem, sortNodes, STUDY_LEVELS, toast, $ } from '../app.js';

const id = pathId();
const mePromise = layout('browse');

// 公共课 courses keep the four fixed folders of the scheme.
const BUCKETS = [[1, '01 真题与答案'], [2, '02 提纲笔记'], [3, '03 题库刷题'], [4, '04 课件与拓展']];
let data = null;
const params = new URLSearchParams(location.search);
let bucket = Number(params.get('b')) || 0;
// 适用专业 filter ('' = all, '-' = files not marked for a major).
let major = '';

/** Keeps the bucket / time filters in the address bar so a refresh keeps them. */
function syncUrl() {
  const u = new URLSearchParams();
  if (bucket) u.set('b', bucket);
  if ($('#time').value) u.set('t', $('#time').value);
  const qs = u.toString();
  history.replaceState(history.state, '', `${location.pathname}${qs ? `?${qs}` : ''}`);
}

let staff = false;

function renderList() {
  const time = $('#time').value;
  const rs = data.resources.filter((r) => (!bucket || r.tag.bucket === bucket) && (!time || r.name.time === time)
    && (!major || (major === '-' ? !r.major : r.major === major)));
  const bar = rs.length > 1 || staff ? `<div class="selbar"><label class="small"><input type="checkbox" id="selall"> 全选</label>
    <button class="btn sm" id="dlsel" type="button" disabled>批量下载</button>
    ${staff ? '<button class="btn sm" id="mvsel" type="button">移动选中到其他分类</button>' : ''}<span class="small faint" id="selcount"></span></div>` : '';
  $('#list').innerHTML = rs.length
    ? bar + rs.map((r) => resourceItem(r, { showNode: false })
      .replace('<div class="item">', `<div class="item"><input type="checkbox" class="rsel" data-id="${r.id}" data-size="${r.size}" aria-label="选择">`)).join('')
    : `<div class="empty"><b>这里还没有资料</b><a href="/upload?node=${data.node.id}">上传第一份</a> · <a href="/wants?node=${data.node.id}">去求资料</a></div>`;
}

async function load() {
  try {
    data = await api(`/nodes/${id}`);
  } catch (e) {
    $('#name').textContent = e.status === 404 ? '分类不存在' : '加载失败';
    $('#list').innerHTML = `<div class="notice bad">${esc(e.message)}</div>`;
    return;
  }
  const n = data.node;
  if (n.id !== id) history.replaceState(history.state, '', `/n/${n.id}${location.search}`);
  document.title = `${n.name} · 鹭岛书阁`;
  const up = data.path.length ? `/n/${data.path[data.path.length - 1].id}` : '/browse';
  $('#crumbs').innerHTML = `<a class="uplevel" title="回到上一页" href="${up}">‹ 返回</a>`
    + ['<a href="/browse">分类</a>', ...data.path.map((p) => `<a href="/n/${p.id}">${esc(p.name)}</a>`)].join(' / ');
  $('#name').innerHTML = esc(nodeTitle(n)) + levelBadge(n);
  $('#info').innerHTML = [
    n.code && `<span class="mono">${esc(n.code)}</span>`,
    `${n.count} 份资料`,
    n.aliases.length && `又名 ${n.aliases.map(esc).join('、')}`,
    n.status === 'pending' && '<span class="badge pending">待确认</span>',
  ].filter(Boolean).join(' · ');
  $('#upload').href = `/upload?node=${n.id}`;
  $('#wantlink').href = `/wants?node=${n.id}`;
  $('#wantlink').hidden = n.kind === 'section';
  $('#within').value = n.id;
  $('#sq').placeholder = `在「${n.name}」中搜索…`;
  $('#upload').hidden = n.kind === 'section';

  sortNodes(data.children);
  $('#children').innerHTML = data.children.map((c) => nodeCard(c).replace('class="card node-card"', `class="card node-card${c.count ? '' : ' empty-node'}" data-level="${c.level || 0}"`)).join('');
  $('#children').hidden = !data.children.length;
  levelChips(data.children);

  const hasRes = data.resources.length > 0;
  $('#rescard').hidden = !hasRes && data.children.length > 0;
  $('#bar').hidden = !hasRes;
  if (hasRes) {
    const counts = new Map();
    for (const r of data.resources) counts.set(r.tag.bucket, (counts.get(r.tag.bucket) || 0) + 1);
    const chips = n.bucketed
      ? [[0, '全部'], ...BUCKETS.filter(([b]) => counts.get(b))]
      : [];
    $('#buckets').innerHTML = chips.map(([b, l]) => `<button class="chip${b === bucket ? ' on' : ''}" data-b="${b}">${l}${b ? ` ${counts.get(b)}` : ''}</button>`).join('');
    const times = [...new Set(data.resources.map((r) => r.name.time).filter(Boolean))].sort().reverse();
    const wantTime = $('#time').value || params.get('t') || '';
    $('#time').innerHTML = '<option value="">全部时间</option>' + times.map((t) => `<option${t === wantTime ? ' selected' : ''}>${esc(t)}</option>`).join('');
    $('#time').hidden = !times.length;
  }
  majorChips();
  renderList();

  const me = await mePromise;
  if (me && me.level >= 3) {
    renderEditor(n);
    if (!staff) { staff = true; renderList(); }
  }
}

$('#buckets').onclick = (e) => {
  const b = e.target.closest('[data-b]');
  if (!b) return;
  bucket = Number(b.dataset.b);
  document.querySelectorAll('#buckets .chip').forEach((x) => x.classList.toggle('on', x === b));
  syncUrl();
  renderList();
};
$('#time').onchange = () => { syncUrl(); renderList(); };
$('#list').addEventListener('change', (e) => {
  if (e.target.id === 'selall') document.querySelectorAll('#list .rsel').forEach((c) => { c.checked = e.target.checked; });
  const sel = [...document.querySelectorAll('#list .rsel:checked')];
  const size = sel.reduce((a, c) => a + Number(c.dataset.size), 0);
  const c = $('#selcount');
  if (c) c.textContent = sel.length ? `已选 ${sel.length} 份 · ${fmtSize(size)}` : '';
  const d = $('#dlsel');
  if (d && !dlBusy) d.disabled = !sel.length;
});

// 批量下载: one file after another, each straight from the mirrors like the 下载 button
// (the server only hands out the links). Files are held in memory one at a time.
const DL_MAX = 30;
const DL_MAX_BYTES = 2 * 1024 ** 3;
let dlBusy = false;
$('#list').addEventListener('click', async (e) => {
  const b = e.target.closest('#dlsel');
  if (!b || dlBusy) return;
  const sel = [...document.querySelectorAll('#list .rsel:checked')];
  const size = sel.reduce((a, c) => a + Number(c.dataset.size), 0);
  if (sel.length > DL_MAX) return toast(`一次最多下载 ${DL_MAX} 份，请少选一些`, true);
  if (size > DL_MAX_BYTES) return toast(`一次最多下载 ${fmtSize(DL_MAX_BYTES)}，请少选一些`, true);
  dlBusy = true;
  b.disabled = true;
  toast('浏览器问「是否允许下载多个文件」时请选允许');
  const failed = [];
  for (const [i, c] of sel.entries()) {
    const id = Number(c.dataset.id);
    try {
      const res = await downloadResource(id, (p) => { b.textContent = `下载中 ${i + 1}/${sel.length} · ${Math.round(p * 100)}%`; }, { batch: true });
      if (!res.ok) failed.push({ id, plan: res.plan });
    } catch { failed.push({ id, plan: null }); }
  }
  b.textContent = '批量下载';
  b.disabled = false;
  dlBusy = false;
  if (failed.length) dlFailed(failed, sel.length);
  else toast(`已下载 ${sel.length} 份`);
});
/** Files no mirror would hand to the page: a link each (a click lets the browser download it
 * from the mirror itself); split files go to their own page, which joins the volumes. */
function dlFailed(failed, total) {
  const body = modal(`${failed.length} / ${total} 份没能自动下载`);
  body.innerHTML = `<p class="small muted">能自动下载的已经保存了。下面这些请逐个点击下载：</p><div class="list">${failed.map(({ id, plan }) => {
    const name = plan ? esc(plan.filename) : `资料 #${id}`;
    const urls = plan && plan.parts.length === 1 ? plan.parts[0].urls : null;
    return `<div class="item"><div class="body"><b>${name}</b>${plan ? ` <span class="small faint">${fmtSize(plan.size)}</span>` : ''}</div>
      ${urls ? `<a class="btn sm primary" href="${esc(urls[0])}" target="_blank" rel="noopener noreferrer">下载</a><a class="small" href="${esc(urls[urls.length - 1])}" target="_blank" rel="noopener noreferrer">GitHub 直链</a>` : `<a class="btn sm" href="/r/${id}" target="_blank">去资料页下载</a>`}</div>`;
  }).join('')}</div>`;
}

$('#list').addEventListener('click', async (e) => {
  if (!e.target.closest('#mvsel')) return;
  const ids = [...document.querySelectorAll('#list .rsel:checked')].map((c) => Number(c.dataset.id));
  if (await moveResources(ids)) load();
});

/** 全部 / 本科 / 研究生 above the child cards, when both kinds are among them. */
/** 全部 / 各专业 above the file list, when the same course has files for different majors
 * (计组 for 计算机 and for 软件工程 cover different ground). */
function majorChips() {
  document.getElementById('mjchips')?.remove();
  const majors = [...new Set(data.resources.map((r) => r.major).filter(Boolean))].sort((a, b) => a.localeCompare(b, 'zh-CN'));
  const unmarked = data.resources.some((r) => !r.major);
  if (majors.length + (unmarked ? 1 : 0) < 2) { major = ''; return; }
  if (major && major !== '-' && !majors.includes(major)) major = '';
  const bar = document.createElement('div');
  bar.id = 'mjchips';
  bar.className = 'chips';
  bar.style.marginBottom = '12px';
  const opts = [['', '全部专业'], ...majors.map((m) => [m, m]), ...(unmarked ? [['-', '未标注专业']] : [])];
  bar.innerHTML = opts.map(([k, l]) => `<button class="chip${k === major ? ' on' : ''}" data-mj="${esc(k)}" type="button">${esc(l)}</button>`).join('');
  $('#list').before(bar);
  bar.onclick = (e) => {
    const b = e.target.closest('[data-mj]');
    if (!b) return;
    major = b.dataset.mj;
    bar.querySelectorAll('.chip').forEach((x) => x.classList.toggle('on', x === b));
    renderList();
  };
}

function levelChips(children) {
  document.getElementById('lvchips')?.remove();
  const grad = (c) => c.level === 2 || c.level === 3;
  const ug = (c) => c.level !== 2;
  if (!children.some(grad) || !children.some(ug)) return;
  const bar = document.createElement('div');
  bar.id = 'lvchips';
  bar.className = 'chips';
  bar.style.marginBottom = '12px';
  bar.innerHTML = [['', '全部'], ['ug', '本科'], ['grad', '研究生']].map(([k, l]) => `<button class="chip${k ? '' : ' on'}" data-lv="${k}" type="button">${l}</button>`).join('');
  $('#children').before(bar);
  bar.onclick = (e) => {
    const b = e.target.closest('[data-lv]');
    if (!b) return;
    bar.querySelectorAll('.chip').forEach((x) => x.classList.toggle('on', x === b));
    for (const card of $('#children').children) {
      const l = Number(card.dataset.level);
      card.hidden = (b.dataset.lv === 'ug' && l === 2) || (b.dataset.lv === 'grad' && l !== 2 && l !== 3);
    }
  };
}

function renderEditor(n) {
  const box = $('#editor');
  box.hidden = false;
  box.innerHTML = `<details class="card" style="margin-bottom:16px"><summary><b>管理这个分类</b> <span class="small muted">（审核员）</span></summary>
    <div style="margin-top:14px">
      <div class="fields-3">
        <label class="field"><span>名称</span><input class="input" id="e_name" value="${esc(n.name)}"></label>
        <label class="field"><span>命名用课程名（文件名第一段）</span><input class="input" id="e_label" value="${esc(n.label)}"></label>
        <label class="field"><span>代号</span><input class="input" id="e_code" value="${esc(n.code)}"></label>
      </div>
      <label class="field"><span>别名 / 俗称（逗号分隔，用于搜索）</span><input class="input" id="e_alias" value="${esc(n.aliases.join('，'))}"></label>
      <label class="field"><span>本科 / 研究生（设在学院或分组上时，下面的课程都跟着它；未设置的算本科）</span><select class="input" id="e_level" style="max-width:320px">
        <option value="0">${n.own_level ? '不单独设置（跟随上级）' : `不单独设置（现在：${STUDY_LEVELS[n.level] || '未标注，算本科'}）`}</option>
        ${[1, 2, 3].map((l) => `<option value="${l}"${n.own_level === l ? ' selected' : ''}>${l === 3 ? '本研都有' : STUDY_LEVELS[l]}</option>`).join('')}</select></label>
      <div class="row"><label class="small"><input type="checkbox" id="e_bucket"${n.bucketed ? ' checked' : ''}> 按 01–04 类型目录分组显示</label>
        <button class="btn primary sm" id="e_save">保存${n.status === 'pending' ? '并确认' : ''}</button></div>
      <hr style="border:0;border-top:1px solid var(--line);margin:16px 0">
      <div class="row">
        <input class="input" id="c_name" placeholder="新建下级：名称" style="max-width:200px">
        <select class="input" id="c_kind" style="max-width:120px"><option value="course">课程</option><option value="level">层次</option><option value="group">分组</option></select>
        <button class="btn sm" id="c_go">新建下级</button>
        <span class="grow"></span>
        <input class="input" id="p_to" placeholder="移到上级分类 ID" style="max-width:130px">
        <button class="btn sm" id="p_go">移动</button>
        <input class="input" id="m_into" placeholder="合并到分类 ID" style="max-width:130px">
        <button class="btn danger sm" id="m_go">合并</button>
        <button class="btn danger sm" id="d_go">删除（仅空分类）</button>
      </div>
      <p class="small faint" style="margin:8px 0 0">分类 ID：${n.id}</p>
    </div></details>`;
  const call = async (fn, ok) => { try { await fn(); toast(ok); load(); } catch (e) { toast(e.message, true); } };
  $('#e_save').onclick = () => call(() => api(`/nodes/${n.id}`, {
    method: 'PATCH',
    body: {
      name: $('#e_name').value, label: $('#e_label').value, code: $('#e_code').value, bucketed: $('#e_bucket').checked, level: Number($('#e_level').value),
      aliases: $('#e_alias').value.split(/[,，、]/).map((s) => s.trim()).filter(Boolean), approve: true,
    },
  }), '已保存');
  $('#c_go').onclick = () => call(() => api('/nodes', { method: 'POST', body: { parent: n.id, kind: $('#c_kind').value, name: $('#c_name').value, bucketed: n.bucketed } }), '已新建');
  $('#p_go').onclick = () => {
    const to = Number($('#p_to').value);
    if (!to) return toast('填写新的上级分类 ID', true);
    call(() => api(`/nodes/${n.id}`, { method: 'PATCH', body: { parent: to } }), '已移动（连同下级分类和资料）');
  };
  $('#m_go').onclick = async () => {
    const into = Number($('#m_into').value);
    if (!into) return toast('填写目标分类 ID', true);
    try {
      const t = await api(`/nodes/${n.id}/merge`, { method: 'POST', body: { into } });
      location.href = `/n/${t.id}`;
    } catch (e) { toast(e.message, true); }
  };
  $('#d_go').onclick = async () => {
    try {
      await api(`/nodes/${n.id}`, { method: 'DELETE' });
      location.href = data.path.length ? `/n/${data.path[data.path.length - 1].id}` : '/browse';
    } catch (e) { toast(e.message, true); }
  };
}

load();
