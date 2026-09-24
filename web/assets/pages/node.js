import { api, esc, layout, moveResources, nodeCard, nodeTitle, pathId, resourceItem, toast, $ } from '../app.js';

const id = pathId();
const mePromise = layout('browse');

// 公共课 courses keep the four fixed folders of the scheme.
const BUCKETS = [[1, '01 真题与答案'], [2, '02 提纲笔记'], [3, '03 题库刷题'], [4, '04 课件与拓展']];
let data = null;
const params = new URLSearchParams(location.search);
let bucket = Number(params.get('b')) || 0;

/** Keeps the bucket / time filters in the address bar so a refresh keeps them. */
function syncUrl() {
  const u = new URLSearchParams();
  if (bucket) u.set('b', bucket);
  if ($('#time').value) u.set('t', $('#time').value);
  const qs = u.toString();
  history.replaceState(null, '', `${location.pathname}${qs ? `?${qs}` : ''}`);
}

let staff = false;

function renderList() {
  const time = $('#time').value;
  const rs = data.resources.filter((r) => (!bucket || r.tag.bucket === bucket) && (!time || r.name.time === time));
  const bar = staff && rs.length ? `<div class="selbar"><label class="small"><input type="checkbox" id="selall"> 全选</label>
    <button class="btn sm" id="mvsel" type="button">移动选中到其他分类</button><span class="small faint" id="selcount"></span></div>` : '';
  $('#list').innerHTML = rs.length
    ? bar + rs.map((r) => {
      const item = resourceItem(r, { showNode: false });
      return staff ? item.replace('<div class="item">', `<div class="item"><input type="checkbox" class="rsel" data-id="${r.id}" aria-label="选择">`) : item;
    }).join('')
    : `<div class="empty"><b>这里还没有资料</b><a href="/upload?node=${data.node.id}">上传第一份</a></div>`;
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
  if (n.id !== id) history.replaceState(null, '', `/n/${n.id}${location.search}`);
  document.title = `${n.name} · 鹭岛书阁`;
  $('#crumbs').innerHTML = ['<a href="/browse">分类</a>', ...data.path.map((p) => `<a href="/n/${p.id}">${esc(p.name)}</a>`)].join(' / ');
  $('#name').textContent = nodeTitle(n);
  $('#info').innerHTML = [
    n.code && `<span class="mono">${esc(n.code)}</span>`,
    `${n.count} 份资料`,
    n.aliases.length && `又名 ${n.aliases.map(esc).join('、')}`,
    n.status === 'pending' && '<span class="badge pending">待确认</span>',
  ].filter(Boolean).join(' · ');
  $('#upload').href = `/upload?node=${n.id}`;
  $('#within').value = n.id;
  $('#sq').placeholder = `在「${n.name}」中搜索…`;
  $('#upload').hidden = n.kind === 'section';

  $('#children').innerHTML = data.children.map((c) => nodeCard(c).replace('class="card node-card"', `class="card node-card${c.count ? '' : ' empty-node'}"`)).join('');
  $('#children').hidden = !data.children.length;

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
  const n = document.querySelectorAll('#list .rsel:checked').length;
  const c = $('#selcount');
  if (c) c.textContent = n ? `已选 ${n} 份` : '';
});
$('#list').addEventListener('click', async (e) => {
  if (!e.target.closest('#mvsel')) return;
  const ids = [...document.querySelectorAll('#list .rsel:checked')].map((c) => Number(c.dataset.id));
  if (await moveResources(ids)) load();
});

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
      name: $('#e_name').value, label: $('#e_label').value, code: $('#e_code').value, bucketed: $('#e_bucket').checked,
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
