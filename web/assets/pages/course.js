import { api, esc, layout, meta, pathId, resourceItem, toast, $ } from '../app.js';

const id = pathId();
const mePromise = layout('courses');

let data = null;
let kind = '';

function renderList() {
  const year = $('#year').value;
  const rs = data.resources.filter((r) => (!kind || r.kind === kind) && (!year || String(r.year) === year));
  $('#list').innerHTML = rs.length
    ? rs.map((r) => resourceItem(r, { showCourse: false })).join('')
    : `<div class="empty"><b>这里还没有资料</b><a href="/upload?course=${id}">上传第一份</a></div>`;
}

async function load() {
  try {
    data = await api(`/courses/${id}`);
  } catch (e) {
    $('#name').textContent = e.status === 404 ? '课程不存在' : '加载失败';
    $('#list').innerHTML = `<div class="notice bad">${esc(e.message)}</div>`;
    return;
  }
  const c = data.course;
  if (c.id !== id) history.replaceState(null, '', `/c/${c.id}`);
  document.title = `${c.name} · XMUHub`;
  $('#crumb').textContent = c.name;
  $('#name').textContent = c.name;
  $('#info').innerHTML = [
    c.code && `<span class="mono">${esc(c.code)}</span>`,
    c.college && esc(c.college),
    `${data.resources.length} 份资料`,
    c.aliases.length && `又名 ${c.aliases.map(esc).join('、')}`,
    c.status === 'pending' && '<span class="badge pending">待确认</span>',
  ].filter(Boolean).join(' · ');
  $('#upload').href = `/upload?course=${c.id}`;

  const m = await meta();
  const present = new Set(data.resources.map((r) => r.kind));
  $('#kinds').innerHTML = [['', '全部'], ...m.kinds.filter((k) => present.has(k.key)).map((k) => [k.key, k.label])]
    .map(([k, l]) => `<button class="chip${k === kind ? ' on' : ''}" data-k="${k}">${l}</button>`).join('');
  const years = [...new Set(data.resources.map((r) => r.year).filter(Boolean))].sort((a, b) => b - a);
  $('#year').innerHTML = '<option value="">全部年份</option>' + years.map((y) => `<option>${y}</option>`).join('');
  renderList();

  const me = await mePromise;
  if (me.level >= 3) renderEditor(c);
}

$('#kinds').onclick = (e) => {
  const b = e.target.closest('[data-k]');
  if (!b) return;
  kind = b.dataset.k;
  document.querySelectorAll('#kinds .chip').forEach((x) => x.classList.toggle('on', x === b));
  renderList();
};
$('#year').onchange = renderList;

function renderEditor(c) {
  const box = $('#editor');
  box.hidden = false;
  box.innerHTML = `<details class="card" style="margin-bottom:16px"><summary><b>管理这门课程</b> <span class="small muted">（审核员）</span></summary>
    <div style="margin-top:14px">
      <div class="fields-3">
        <label class="field"><span>课程名称</span><input class="input" id="e_name" value="${esc(c.name)}"></label>
        <label class="field"><span>课程代码</span><input class="input" id="e_code" value="${esc(c.code)}"></label>
        <label class="field"><span>开课学院</span><input class="input" id="e_college" value="${esc(c.college)}"></label>
      </div>
      <label class="field"><span>别名（逗号分隔，用于搜索）</span><input class="input" id="e_alias" value="${esc(c.aliases.join('，'))}"></label>
      <div class="row"><button class="btn primary sm" id="e_save">保存${c.status === 'pending' ? '并确认课程' : ''}</button></div>
      <hr style="border:0;border-top:1px solid var(--line);margin:16px 0">
      <div class="row"><span class="small muted">合并到另一门课程（本课程的资料全部移过去）：</span>
        <input class="input" id="m_into" placeholder="目标课程 ID" style="max-width:140px">
        <button class="btn danger sm" id="m_go">合并</button></div>
    </div></details>`;
  $('#e_save').onclick = async () => {
    try {
      await api(`/courses/${c.id}`, {
        method: 'PATCH',
        body: {
          name: $('#e_name').value, code: $('#e_code').value, college: $('#e_college').value,
          aliases: $('#e_alias').value.split(/[,，、]/).map((s) => s.trim()).filter(Boolean), approve: true,
        },
      });
      toast('已保存');
      load();
    } catch (e) { toast(e.message, true); }
  };
  $('#m_go').onclick = async () => {
    const into = Number($('#m_into').value);
    if (!into) return toast('请填写目标课程 ID', true);
    try {
      const t = await api(`/courses/${c.id}/merge`, { method: 'POST', body: { into } });
      toast(`已合并到「${t.name}」`);
      location.href = `/c/${t.id}`;
    } catch (e) { toast(e.message, true); }
  };
}

load();
