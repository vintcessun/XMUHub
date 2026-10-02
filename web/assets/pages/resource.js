import { ago, api, avatar, downloadResource, esc, favoriteDialog, fmtDate, fmtSize, layout, linkify, loginUrl, meta, pathId, pickFromTree, preview, seriesAbout, seriesEditor, SITE_NAME, stars, statusBadge, store, toast, $ } from '../app.js';

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
  document.title = `${r.title} · ${SITE_NAME}`;
  $('#crumbs').innerHTML = `<a class="uplevel" title="回到上一页" href="/n/${r.node.id}">‹ 返回</a>` + ['<a href="/browse">分类</a>', ...r.path.map((p) => `<a href="/n/${p.id}">${esc(p.name)}</a>`), `<a href="/n/${r.node.id}">${esc(r.node.name)}</a>`].join(' / ');
  $('#title').textContent = r.title;
  $('#subtitle').textContent = r.subtitle || '';
  $('#subtitle').hidden = !r.subtitle;
  $('#tags').innerHTML = `<span class="tag t-${esc(r.tag.code)}">${esc(r.tag.code)} ${esc(r.tag.label)}</span>${statusBadge(r)}<span>${r.downloads} 次下载</span>`;
  $('#note').innerHTML = linkify(r.note || '');
  $('#note').hidden = !r.note;
  const n = r.name;
  const detail = [n.paper, n.with_answer && '含答案', n.extra].filter(Boolean).join('、');
  const rows = [
    ['分类', `<a href="/n/${r.node.id}">${esc(r.node.name)}</a>`],
    ['课程', esc(n.course)],
    ['时间', esc(n.time)],
    ['类型', esc(n.type_word) + (detail ? `（${esc(detail)}）` : '')],
    r.major && ['适用专业', esc(r.major)],
    ['文件名', `<span class="mono">${esc(r.filename)}</span>`],
    ['大小', fmtSize(r.size)],
    ['上传于', fmtDate(r.created_at)],
    r.original_name && ['原文件名', `<span class="faint">${esc(r.original_name)}</span>`],
    r.source && ['来源', sourceHtml(r.source)],
    r.uploader && ['上传者', `<span class="who-inline">${avatar(r.uploader.avatar, r.uploader.nickname, 20)}<span class="faint">${esc(r.uploader.nickname)}</span></span>`],
    r.reviewer && ['审核人', `<span class="faint">${esc(r.reviewer.nickname)}</span>`],
  ].filter((x) => x && x[1]);
  $('#kv').innerHTML = rows.map(([k, v]) => `<dt>${k}</dt><dd>${v}</dd>`).join('');
  const notes = {
    pending: ['warn', '这份资料正在等待审核，通过后才会对所有人可见。'],
    rejected: ['bad', `这份资料未通过审核。${r.review_note ? '原因：' + esc(r.review_note) : ''}${r.mine ? '<br>如有疑问可以联系审核员，或修改后重新上传。' : ''}`],
    removed: ['bad', `这份资料已下架。${r.review_note ? '原因：' + esc(r.review_note) : ''}`],
    restricted: ['warn', '这份资料标记为「仅内部」，不对外公开、也不会被搜索到。'],
  };
  const note = notes[r.status];
  $('#notice').innerHTML = (note ? `<div class="notice ${note[0]}">${note[1]}</div>` : '')
    + (r.uncertain ? '<div class="notice">这份资料的分类或内容尚未核实，欢迎审核员确认。</div>' : '')
    + questionsHtml(r);
  bindAnswers(r);
  const me = await mePromise;
  const unavailable = r.status === 'rejected' || (me?.level < 3 && (r.status === 'removed' || r.status === 'restricted'));
  $('#dl').disabled = unavailable;
  // Small previewable files load their preview right away.
  const ext = (r.ext || '').toLowerCase();
  const previewable = !['caj', 'kdh', 'nh', 'exe', 'msi', 'apk', 'dmg'].includes(ext);
  $('#pvcard').hidden = unavailable;
  if (!unavailable && previewable && r.size <= 8 * 1024 * 1024 && !$('#pvgo').hidden) { $('#pvgo').hidden = true; preview(id, $('#pv')); }
  $('#dl').textContent = `下载 · ${fmtSize(r.size)}`;

  $('#fav').hidden = r.status !== 'published';
  if (me) {
    api(`/resources/${id}/collections`).then((ids) => { if (ids.length) $('#fav').textContent = '★ 已收藏'; }).catch(() => {});
  }
  $('#fav').onclick = async () => {
    if (!me) { location.href = loginUrl(); return; }
    try {
      const inSet = await favoriteDialog(id);
      // The dialog updates the set as boxes are ticked; repaint the button when it closes.
      const obs = new MutationObserver(() => {
        if (document.querySelector('.modal')) return;
        obs.disconnect();
        $('#fav').textContent = inSet.size ? '★ 已收藏' : '☆ 收藏';
      });
      obs.observe(document.body, { childList: true });
    } catch (e) { toast(e.message, true); }
  };
  seriesCard(r, me);
  if (me && (me.level >= 3 || r.mine)) renderManage(r, me);
  if (me && me.level >= 3) loadHistory();
  loadSocial(me);
}

// ---------------------------------------------------------------- preview

$('#pvgo').onclick = () => { $('#pvgo').hidden = true; preview(id, $('#pv')); };

// ---------------------------------------------------------------- ratings

const ACTIONS = { edit: '上传者修改了信息', approve: '通过', reject: '驳回', remove: '下架', restrict: '设为仅内部', restore: '恢复发布', note_approved: '同意修改备注', note_rejected: '驳回备注申请', delete_approved: '同意删除', delete_rejected: '驳回删除申请' };

async function loadHistory() {
  try {
    const list = await api(`/resources/${id}/reviews`);
    if (!list.length) return;
    $('#history').hidden = false;
    $('#hlist').innerHTML = list.map((e) => `<div style="padding:4px 0"><b>${esc(e.actor)}</b> ${ACTIONS[e.action] || esc(e.action)}
      <span class="faint">· ${fmtDate(e.at, true)}</span>${e.note ? ` <span class="muted">· ${esc(e.note)}</span>` : ''}</div>`).join('');
  } catch { /* staff only */ }
}

let social = null;
async function loadSocial(me) {
  try { social = await api(`/resources/${id}/social`); } catch { $('#social').hidden = true; return; }
  const s = social;
  $('#ravg').innerHTML = s.rating.count ? `${stars(s.rating.avg)} <b>${s.rating.avg}</b> · ${s.rating.count} 人评分` : '还没有评分';
  $('#rate').innerHTML = [1, 2, 3, 4, 5].map((n) => `<button type="button" data-s="${n}" class="${n <= s.my_rating ? 'on' : ''}" aria-label="${n} 星">★</button>`).join('');
  $('#ratehint').innerHTML = !me ? `<a href="${loginUrl()}">登录</a>后评分` : s.my_rating ? '再点一次同一颗星可取消' : '';
  $('#rate').onclick = async (e) => {
    const b = e.target.closest('[data-s]');
    if (!b) return;
    if (!me) { location.href = loginUrl(); return; }
    const n = Number(b.dataset.s);
    try {
      await api(`/resources/${id}/rating`, { method: 'PUT', body: { stars: n === s.my_rating ? 0 : n } });
      loadSocial(me);
    } catch (err) { toast(err.message, true); }
  };
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

// 建议换个分类: pick the right course; a reviewer moves the file after checking.
$('#suggestmove').onclick = async (e) => {
  e.preventDefault();
  const user = await mePromise;
  if (!user) { location.href = loginUrl(); return; }
  const picked = await pickFromTree('这份资料应该放在哪门课程');
  if (!picked) return;
  const note = prompt(`建议移到「${picked.path.map((n) => n.name).concat(picked.node.name).join(' / ')}」。
可以说一下原因（选填）：`);
  if (note === null) return;
  try {
    await api(`/resources/${id}/move-suggestion`, { method: 'POST', body: { node: picked.node.id, note } });
    toast('已提交，审核员确认后会移过去，结果会在站内提醒里告诉你');
  } catch (err) { toast(err.message, true); }
};

$('#report').onclick = async (e) => {
  e.preventDefault();
  const user = await mePromise;
  // Complaints come from signed-in users only (the reply goes to the account email).
  if (!user) { toast('投诉和申请下架需要先登录'); location.href = loginUrl(); return; }
  const reason = prompt(`请写明投诉或申请下架的理由（例如：侵犯版权 / 含个人隐私）。
以权利人身份申请下架的，请写明你的身份，并说明能证明资料属于你的材料（原始文件或手稿、编写记录、署名或出版信息等）；没有证明材料的一律驳回。
核实和补充材料会通过你的账号邮箱 ${user.email} 联系你。
（只是分类放错了，请用「建议换个分类」。）`);
  if (!reason) return;
  try {
    await api(`/resources/${id}/report`, { method: 'POST', body: { reason } });
    toast('已收到，我们会在 48 小时内处理');
  } catch (err) { toast(err.message, true); }
};

/** 合集 this file is in: 第 3 / 12 份, the neighbours, and the whole list, like a video 合集. */
function seriesCard(r, me) {
  const box = $('#series');
  const s = r.series;
  box.hidden = !s;
  if (!s) return;
  const at = s.items.findIndex((i) => i.id === r.id);
  const prev = s.items[at - 1];
  const next = s.items[at + 1];
  box.innerHTML = `<div class="row"><b>合集</b><span class="grow"></span><span class="small muted">第 ${at + 1} / ${s.items.length} 份</span></div>
    <p class="series-name">${esc(s.title)}${seriesAbout(s) ? `<span class="small muted" style="display:block;font-weight:400">${seriesAbout(s)}</span>` : ''}</p>
    <div class="row series-nav">${prev ? `<a class="btn sm" href="/r/${prev.id}" title="${esc(prev.title)}">‹ 上一份</a>` : '<span class="btn sm" aria-disabled="true">‹ 上一份</span>'}
      ${next ? `<a class="btn sm primary" href="/r/${next.id}" title="${esc(next.title)}">下一份 ›</a>` : '<span class="btn sm" aria-disabled="true">下一份 ›</span>'}</div>
    <ol class="series-list">${s.items.map((i, k) => `<li${i.id === r.id ? ' class="on" aria-current="true"' : ''}><span class="series-no">${k + 1}</span>
      ${i.id === r.id ? `<b>${esc(i.title)}</b>` : `<a href="/r/${i.id}">${esc(i.title)}</a>`}</li>`).join('')}</ol>
    ${me ? '<p class="small" style="margin:8px 0 0"><a href="#" id="sedit">修改合集</a></p>' : ''}`;
  const cur = box.querySelector('li.on');
  const list = box.querySelector('.series-list');
  if (cur) list.scrollTop = cur.offsetTop - list.offsetTop - list.clientHeight / 2 + cur.clientHeight / 2;
  const ed = box.querySelector('#sedit');
  if (ed) ed.onclick = async (e) => { e.preventDefault(); if (await seriesEditor(r.node.id, s)) load(); };
}

async function renderManage(r, me) {
  const box = $('#manage');
  box.hidden = false;
  // On their own files reviewers are uploaders like anyone: someone else reviews the file,
  // its edits and their note / removal requests. Admins are exempt.
  const staff = me.level >= 4 || (me.level >= 3 && !r.mine);
  // Uploaders edit their pending and published files (a published one goes back to review).
  const canEdit = staff || r.status === 'pending' || r.status === 'published';
  const m = canEdit ? await meta() : null;
  const request = !staff && r.mine ? await api(`/resources/${id}/change-request`) : null;
  const pendingRequest = request?.status === 'pending';
  const actions = staff
    ? [
        (r.status !== 'published' || r.needs_review || r.uncertain) && ['approve', r.status === 'published' ? '确认无误' : '通过并公开', 'ok'],
        (r.status === 'pending' || r.status === 'restricted' || r.needs_review || r.uncertain) && ['reject', '驳回', 'danger'],
        r.status === 'published' && ['remove', '下架', 'danger'],
        r.status !== 'restricted' && ['restrict', '设为仅内部', ''],
        (r.status === 'removed' || r.status === 'rejected' || r.status === 'restricted') && ['restore', '恢复发布', ''],
      ].filter(Boolean)
    : [];
  const n = r.name;
  // What saving does for the uploader (see update_resource on the server).
  const hint = staff ? '' : r.status === 'pending' ? '审核通过前可以随时修改。'
    : me.level >= 2 ? '保存后立即生效，审核员会再复核一遍。' : '保存后会重新提交审核，审核通过前暂时不公开。';
  let nodeId = r.node.id;
  box.innerHTML = `<h3>${staff ? '审核' : '管理我的投稿'}</h3>
    ${actions.length ? `<label class="field"><span>备注（驳回 / 下架原因）</span><input class="input" id="review_note"></label>
      <div class="row" style="margin-bottom:16px">${actions.map(([a, l, c]) => `<button class="btn sm ${c}" data-a="${a}">${l}</button>`).join('')}</div>` : ''}
    ${!staff && r.mine ? `<div class="notice" style="margin-bottom:14px"><b>备注与删除申请</b><p class="small">申请由审核员同意后生效；删除获批后资料会下架。</p>
      ${request ? `<p class="small">最近申请：${request.kind === 'note' ? '修改备注' : '删除资料'} · ${request.status === 'pending' ? '待审核' : request.status === 'approved' ? '已同意' : '已驳回'}${request.review_note ? ` · 审核意见：${esc(request.review_note)}` : ''}</p>` : ''}
      ${pendingRequest ? '' : r.status === 'removed' || r.status === 'rejected' ? '<p class="small">这份资料当前不能提交新申请。</p>' : `<label class="field"><span>新备注（可留空以清除）</span><textarea class="input" id="change_note" maxlength="500">${esc(r.note)}</textarea></label>
        <div class="row"><button class="btn sm" id="request_note" type="button">申请修改备注</button></div>
        <label class="field" style="margin-top:12px"><span>删除理由</span><input class="input" id="delete_reason" maxlength="300" placeholder="请说明为什么要删除这份资料"></label>
        <button class="btn sm danger" id="request_delete" type="button">申请删除资料</button>`}
    </div>` : ''}
    ${canEdit ? `<details${staff ? '' : ' open'}><summary class="small">编辑信息</summary><div style="margin-top:12px">
      ${hint ? `<p class="small muted" style="margin-top:0">${hint}</p>` : ''}
      <label class="field"><span>小标题（公开显示，说明具体内容；默认取原文件名，看不懂就改掉，清空则不显示）</span><input class="input" id="f_sub" maxlength="80" value="${esc(r.subtitle || '')}"></label>
      <label class="field"><span>课程名（文件名第一段）</span><input class="input" id="f_course" value="${esc(n.course)}"></label>
      <div class="fields-2">
        <label class="field"><span>时间</span><input class="input" id="f_time" value="${esc(n.time)}" placeholder="2023-2024秋 / 2025春 / 202406"></label>
        <label class="field"><span>类型</span><select class="input" id="f_type">${m.type_words.map((t) => `<option${t.word === n.type_word ? ' selected' : ''}>${t.word}</option>`).join('')}${m.type_words.some((t) => t.word === n.type_word) ? '' : `<option selected>${esc(n.type_word)}</option>`}</select></label>
        <label class="field"><span>卷别</span><select class="input" id="f_paper"><option value="">无</option>${m.papers.map((p) => `<option${p === n.paper ? ' selected' : ''}>${p}</option>`).join('')}</select></label>
        <label class="field"><span>标签</span><select class="input" id="f_tag">${m.tags.map((t) => `<option value="${t.code}"${t.code === r.tag.code ? ' selected' : ''}>${t.code} ${t.label}</option>`).join('')}</select></label>
      </div>
      <label class="small"><input type="checkbox" id="f_ans"${n.with_answer ? ' checked' : ''}> 含答案</label>
      <label class="field" style="margin-top:8px"><span>补充说明（括号内，如 201题、第1册）</span><input class="input" id="f_extra" value="${esc(n.extra)}"></label>
      <label class="field"><span>适用专业（选填：同一门课不同专业考的不一样时填，如 软件工程；清空则不标注）</span><input class="input" id="f_major" maxlength="20" value="${esc(r.major || '')}"></label>
      ${staff ? `<label class="field"><span>备注（公开显示）</span><textarea class="input" id="f_note">${esc(r.note)}</textarea></label>` : ''}
      ${staff ? `<div class="field"><span>所属分类</span><div class="row"><span id="f_nodename">${esc(r.node.name)}</span><span class="small faint">ID</span><input class="input" id="f_node" value="${r.node.id}" style="max-width:110px"><button class="btn sm" id="f_pick" type="button">选择分类</button></div></div>
        <label class="small"><input type="checkbox" id="f_unc"${r.uncertain ? ' checked' : ''}> 待核实</label>`
        : `<div class="field"><span>所属分类</span><div class="row"><span id="f_nodename">${esc(r.node.name)}</span><button class="btn sm" id="f_pick" type="button">换一个分类</button></div></div>`}
      <div style="margin-top:10px"><button class="btn primary sm" id="f_save">保存</button></div>
    </div></details>` : ''}`;
  if (!staff && r.mine && !pendingRequest && r.status !== 'removed' && r.status !== 'rejected') {
    const send = async (kind, value) => {
      try {
        await api(`/resources/${id}/change-request`, { method: 'POST', body: { kind, value } });
        toast('申请已提交，等待审核员处理');
        load();
      } catch (e) { toast(e.message, true); }
    };
    $('#request_note').onclick = () => send('note', $('#change_note').value);
    $('#request_delete').onclick = () => send('delete', $('#delete_reason').value);
  }
  box.querySelectorAll('[data-a]').forEach((b) => {
    b.onclick = async () => {
      try {
        const res = await api(`/resources/${id}/review`, { method: 'POST', body: { action: b.dataset.a, note: $('#review_note').value } });
        toast(`已处理：${res.status}`);
        load();
      } catch (e) { toast(e.message, true); }
    };
  });
  if (canEdit) {
    $('#f_pick').onclick = async () => {
      const t = (await pickFromTree('把这份资料放到哪个分类'))?.node;
      if (!t) return;
      nodeId = t.id;
      $('#f_nodename').textContent = t.name;
      if (staff) $('#f_node').value = t.id;
    };
  }
  if (canEdit) $('#f_save').onclick = async () => {
    try {
      await api(`/resources/${id}`, {
        method: 'PATCH',
        body: {
          node: staff ? Number($('#f_node').value) : nodeId,
          course: $('#f_course').value, time: $('#f_time').value, type_word: $('#f_type').value, tag: $('#f_tag').value,
          paper: $('#f_paper').value, with_answer: $('#f_ans').checked, extra: $('#f_extra').value, note: staff ? $('#f_note').value : r.note, subtitle: $('#f_sub').value, major: $('#f_major').value,
          admin: staff ? { uncertain: $('#f_unc').checked, free_type: true } : null,
        },
      });
      toast(!staff && r.status === 'published' && me.level < 2 ? '已保存，等待审核' : '已保存');
      load();
    } catch (e) { toast(e.message, true); }
  };
}

/** Reviewers' questions about this file (「问上传者」): the uploader answers them here. */
function questionsHtml(r) {
  if (!r.questions || !r.questions.length) return '';
  return r.questions.map((q) => `<div class="notice${q.answer ? ' ok' : ' warn'}" data-q="${q.id}">
    <b>审核员 ${esc(q.asker)} 问：</b>${esc(q.text)} <span class="small faint">${ago(q.asked_at)}</span>
    ${q.answer ? `<div style="margin-top:6px"><b>上传者答：</b>${esc(q.answer)}</div>`
      : r.mine ? `<div class="row" style="margin-top:8px;flex-wrap:nowrap"><input class="input" maxlength="500" placeholder="回答审核员（回答后资料会继续审核）"><button class="btn sm primary" data-answer type="button">回答</button></div>`
        : '<div class="small faint" style="margin-top:6px">等待上传者回答</div>'}
  </div>`).join('');
}
function bindAnswers(r) {
  $('#notice').querySelectorAll('[data-answer]').forEach((b) => {
    b.onclick = async () => {
      const box = b.closest('[data-q]');
      try {
        await api(`/questions/${box.dataset.q}/answer`, { method: 'POST', body: { text: box.querySelector('input').value } });
        toast('已回答，谢谢！');
        load();
      } catch (e) { toast(e.message, true); }
    };
  });
}

/** Where an imported file came from; a GitHub repo pinned to a commit links to that commit. */
function sourceHtml(src) {
  const m = /^github:([A-Za-z0-9_.-]+)\/([A-Za-z0-9_.-]+)@([0-9a-f]{6,40})$/.exec(src);
  if (!m) return `<span class="faint">${esc(src)}</span>`;
  return `<a href="https://github.com/${m[1]}/${m[2]}/tree/${m[3]}" target="_blank" rel="noopener noreferrer nofollow">GitHub ${esc(m[1])}/${esc(m[2])}</a> <span class="faint mono">@${esc(m[3].slice(0, 8))}</span>`;
}

if (!id) location.href = '/';
load();
