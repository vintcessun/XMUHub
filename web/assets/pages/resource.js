import { api, downloadResource, esc, fmtDate, fmtSize, layout, meta, pathId, statusBadge, toast, $ } from '../app.js';

const id = pathId();
const mePromise = layout('browse');

async function load() {
  let r;
  try {
    r = await api(`/resources/${id}`);
  } catch (e) {
    $('#title').textContent = e.status === 404 ? '资料不存在或尚未通过审核' : '加载失败';
    $('#dl').disabled = true;
    return;
  }
  document.title = `${r.title} · XMUHub`;
  $('#crumbs').innerHTML = ['<a href="/browse">分类</a>', ...r.path.map((p) => `<a href="/n/${p.id}">${esc(p.name)}</a>`), `<a href="/n/${r.node.id}">${esc(r.node.name)}</a>`].join(' / ');
  $('#title').textContent = r.title;
  $('#tags').innerHTML = `<span class="tag t-${esc(r.tag.code)}">${esc(r.tag.code)} ${esc(r.tag.label)}</span>${statusBadge(r)}<span>${r.downloads} 次下载</span>`;
  $('#note').textContent = r.note || '';
  $('#note').hidden = !r.note;
  const n = r.name;
  const detail = [n.paper, n.with_answer && '含答案', n.extra].filter(Boolean).join('、');
  const rows = [
    ['分类', `<a href="/n/${r.node.id}">${esc(r.node.name)}</a>`],
    ['课程', esc(n.course)],
    ['时间', esc(n.time)],
    ['类型', esc(n.type_word) + (detail ? `（${esc(detail)}）` : '')],
    ['文件名', `<span class="mono">${esc(r.filename)}</span>`],
    ['大小', fmtSize(r.size)],
    ['上传于', fmtDate(r.created_at)],
    r.original_name && ['原文件名', `<span class="faint">${esc(r.original_name)}</span>`],
    r.source && ['来源', `<span class="faint">${esc(r.source)}</span>`],
    r.uploader && ['上传者', `<span class="faint">${esc(r.uploader.nickname)}</span>`],
  ].filter((x) => x && x[1]);
  $('#kv').innerHTML = rows.map(([k, v]) => `<dt>${k}</dt><dd>${v}</dd>`).join('');
  const notes = {
    pending: ['warn', '这份资料正在等待审核，通过后才会对所有人可见。'],
    rejected: ['bad', `这份资料未通过审核。${r.review_note ? '原因：' + esc(r.review_note) : ''}`],
    removed: ['bad', `这份资料已下架。${r.review_note ? '原因：' + esc(r.review_note) : ''}`],
    restricted: ['warn', '这份资料标记为「仅内部」，不对外公开、也不会被搜索到。'],
  };
  const note = notes[r.status];
  $('#notice').innerHTML = (note ? `<div class="notice ${note[0]}">${note[1]}</div>` : '')
    + (r.uncertain ? '<div class="notice">这份资料的分类或内容尚未核实，欢迎审核员确认。</div>' : '');
  $('#dl').disabled = r.status === 'rejected';
  $('#dl').textContent = `下载 · ${fmtSize(r.size)}`;

  const me = await mePromise;
  if (me && (me.level >= 3 || (r.mine && r.status === 'pending'))) renderManage(r, me);
}

$('#dl').onclick = async () => {
  const btn = $('#dl');
  const bar = $('#dlprog');
  btn.disabled = true;
  bar.hidden = false;
  const label = btn.textContent;
  btn.textContent = '正在连接镜像…';
  try {
    const res = await downloadResource(id, (p) => {
      bar.firstElementChild.style.width = `${Math.round(p * 100)}%`;
      btn.textContent = `下载中 ${Math.round(p * 100)}%`;
    });
    if (res.ok) {
      btn.textContent = res.fallback ? '已开始下载' : '下载完成';
    } else {
      btn.textContent = label;
      $('#parts').hidden = false;
      $('#parts').innerHTML = '<p class="notice warn" style="margin-top:12px">自动合并失败，请依次下载以下分卷，再按顺序拼接：</p>' +
        res.plan.parts.map((p, i) => `<div><a href="${esc(p.urls[0])}" rel="noreferrer">分卷 ${i + 1}</a> · ${fmtSize(p.size)}</div>`).join('');
    }
  } catch (e) {
    toast(e.message, true);
    btn.textContent = label;
  } finally {
    setTimeout(() => { btn.disabled = false; bar.hidden = true; btn.textContent = label; }, 4000);
  }
};

$('#report').onclick = async (e) => {
  e.preventDefault();
  const reason = prompt('请写明投诉或申请下架的理由（例如：侵犯版权 / 含个人隐私 / 分类错误）：');
  if (!reason) return;
  const contact = prompt('联系方式（选填，方便我们回复你）：') || '';
  try {
    await api(`/resources/${id}/report`, { method: 'POST', body: { reason, contact } });
    toast('已收到，我们会在 48 小时内处理');
  } catch (err) { toast(err.message, true); }
};

async function renderManage(r, me) {
  const box = $('#manage');
  box.hidden = false;
  const m = await meta();
  const staff = me.level >= 3;
  const actions = staff
    ? [
        (r.status !== 'published' || r.needs_review || r.uncertain) && ['approve', r.status === 'published' ? '确认无误' : '通过并公开', 'ok'],
        r.status === 'pending' && ['reject', '驳回', 'danger'],
        r.status === 'published' && ['remove', '下架', 'danger'],
        r.status !== 'restricted' && ['restrict', '设为仅内部', ''],
        (r.status === 'removed' || r.status === 'rejected' || r.status === 'restricted') && ['restore', '恢复发布', ''],
      ].filter(Boolean)
    : [];
  const n = r.name;
  box.innerHTML = `<h3>${staff ? '审核' : '修改我的投稿'}</h3>
    ${actions.length ? `<label class="field"><span>备注（驳回 / 下架原因）</span><input class="input" id="note"></label>
      <div class="row" style="margin-bottom:16px">${actions.map(([a, l, c]) => `<button class="btn sm ${c}" data-a="${a}">${l}</button>`).join('')}</div>` : ''}
    <details><summary class="small">编辑信息</summary><div style="margin-top:12px">
      <label class="field"><span>课程名（文件名第一段）</span><input class="input" id="f_course" value="${esc(n.course)}"></label>
      <div class="fields-2">
        <label class="field"><span>时间</span><input class="input" id="f_time" value="${esc(n.time)}" placeholder="2023-2024秋 / 2025春 / 202406"></label>
        <label class="field"><span>类型</span><select class="input" id="f_type">${m.type_words.map((t) => `<option${t.word === n.type_word ? ' selected' : ''}>${t.word}</option>`).join('')}${m.type_words.some((t) => t.word === n.type_word) ? '' : `<option selected>${esc(n.type_word)}</option>`}</select></label>
        <label class="field"><span>卷别</span><select class="input" id="f_paper"><option value="">无</option>${m.papers.map((p) => `<option${p === n.paper ? ' selected' : ''}>${p}</option>`).join('')}</select></label>
        <label class="field"><span>标签</span><select class="input" id="f_tag">${m.tags.map((t) => `<option value="${t.code}"${t.code === r.tag.code ? ' selected' : ''}>${t.code} ${t.label}</option>`).join('')}</select></label>
      </div>
      <label class="small"><input type="checkbox" id="f_ans"${n.with_answer ? ' checked' : ''}> 含答案</label>
      <label class="field" style="margin-top:8px"><span>补充说明（括号内，如 201题、第1册）</span><input class="input" id="f_extra" value="${esc(n.extra)}"></label>
      <label class="field"><span>备注（公开显示）</span><textarea class="input" id="f_note">${esc(r.note)}</textarea></label>
      ${staff ? `<label class="field"><span>移到分类 ID</span><input class="input" id="f_node" value="${r.node.id}"></label>
        <label class="small"><input type="checkbox" id="f_unc"${r.uncertain ? ' checked' : ''}> 待核实</label>` : ''}
      <div style="margin-top:10px"><button class="btn primary sm" id="f_save">保存</button></div>
    </div></details>`;
  box.querySelectorAll('[data-a]').forEach((b) => {
    b.onclick = async () => {
      try {
        const res = await api(`/resources/${id}/review`, { method: 'POST', body: { action: b.dataset.a, note: $('#note').value } });
        toast(`已处理：${res.status}`);
        load();
      } catch (e) { toast(e.message, true); }
    };
  });
  $('#f_save').onclick = async () => {
    try {
      await api(`/resources/${id}`, {
        method: 'PATCH',
        body: {
          node: staff ? Number($('#f_node').value) : r.node.id,
          course: $('#f_course').value, time: $('#f_time').value, type_word: $('#f_type').value, tag: $('#f_tag').value,
          paper: $('#f_paper').value, with_answer: $('#f_ans').checked, extra: $('#f_extra').value, note: $('#f_note').value,
          admin: staff ? { uncertain: $('#f_unc').checked, free_type: true } : null,
        },
      });
      toast('已保存');
      load();
    } catch (e) { toast(e.message, true); }
  };
}

if (!id) location.href = '/';
load();
