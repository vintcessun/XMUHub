import { api, arrowKeys, esc, fmtSize, layout, loginUrl, meta, modal, pathText, pickFromTree, qs, sendPart, store, toast, tree, $ } from '../app.js';

const state = { node: null, path: [], rows: [], busy: false };
let M = null; // meta

// ---------------------------------------------------------------- draft (survives a refresh)
//
// Browsers never let a page keep the chosen files across a reload, so the draft keeps
// everything else: the category, each file's filled-in fields, its hashes and its server
// upload id. Choosing the same files again restores the fields and resumes the upload
// from the first part that hadn't finished.

const DRAFT_KEY = 'xmuhub.upload.draft';
const DRAFT_TTL = 20 * 3600 * 1000; // the server drops unfinished uploads after 24 h
// `node`: a category chosen for this one file, overriding the one picked for the batch.
const FIELDS = ['type_word', 'year', 'term', 'paper', 'with_answer', 'extra', 'note', 'subtitle', 'node', 'major'];

const fileKey = (f) => `${f.name}|${f.size}|${f.lastModified}`;

function readDraft() {
  try {
    const d = JSON.parse(store.get(DRAFT_KEY) || 'null');
    if (d && Date.now() - d.at < DRAFT_TTL) return d;
  } catch { /* corrupt */ }
  return { at: Date.now(), node: null, files: {} };
}
let draft = readDraft();

function saveDraft() {
  draft.at = Date.now();
  draft.node = state.node ? state.node.id : null;
  const files = {};
  for (const r of state.rows) {
    if (r.done) continue;
    const e = { name: r.file.name, size: r.file.size };
    for (const k of FIELDS) e[k] = r[k];
    if (r.parts) e.parts = r.parts;
    if (r.upload_id) e.upload_id = r.upload_id;
    files[fileKey(r.file)] = e;
  }
  // Files from before the refresh that haven't been chosen again yet stay in the draft.
  for (const [k, v] of Object.entries(draft.files || {})) if (!(k in files) && !state.rows.some((r) => fileKey(r.file) === k)) files[k] = v;
  draft.files = files;
  store.set(DRAFT_KEY, Object.keys(files).length || draft.node ? JSON.stringify(draft) : null);
}

function showDraftNotice() {
  const waiting = Object.entries(draft.files || {}).filter(([k]) => !state.rows.some((r) => fileKey(r.file) === k));
  const box = $('#draftnote');
  if (!waiting.length) { box.hidden = true; return; }
  box.hidden = false;
  box.innerHTML = `<div class="notice warn"><b>上次还有 ${waiting.length} 个文件没有提交</b>（页面刷新或关闭了）。重新选择这些文件，填过的信息会自动恢复，已传完的部分不用重传：
    <div class="small" style="margin-top:6px">${waiting.slice(0, 8).map(([, v]) => `${esc(v.name)} · ${fmtSize(v.size)}`).join('<br>')}${waiting.length > 8 ? `<br>… 等 ${waiting.length} 个` : ''}</div>
    <div style="margin-top:8px"><a href="#" id="draftclear" class="small">不用了，清除记录</a></div></div>`;
  $('#draftclear').onclick = (e) => { e.preventDefault(); draft.files = {}; saveDraft(); showDraftNotice(); };
}

// Warn before leaving with files that haven't been submitted.
window.addEventListener('beforeunload', (e) => {
  if (state.busy || state.rows.some((r) => !r.done)) { e.preventDefault(); e.returnValue = ''; }
});

// ---------------------------------------------------------------- gate

(async () => {
  const me = await layout('upload');
  if (!me) {
    $('#gate').innerHTML = `<section class="card"><h2>请先登录</h2><p class="muted">上传资料需要一个账号，注册只需邮箱验证码。</p>
      <a class="btn primary" href="${loginUrl()}">登录 / 注册</a></section>`;
    return;
  }
  M = await meta();
  $('#form').hidden = false;
  $('#droptip').textContent = `支持任意格式：PDF、Word、PPT、Excel、图片、压缩包等；单个文件最大 ${fmtSize(M.limits.max_file)}，大文件自动分卷上传`;
  $('#b_type').innerHTML += M.type_words.map((t) => `<option>${t.word}</option>`).join('');
  $('#b_year').innerHTML += yearOptions('');
  // Last time's category comes back only to resume unfinished files; a fresh upload starts
  // empty, or a new batch silently lands in whatever course was picked days ago.
  const startNode = Number(qs.get('node')) || (Object.keys(draft.files || {}).length ? draft.node : null);
  if (startNode) {
    try { const d = await api(`/nodes/${startNode}`); pick(d.node, d.path); } catch { /* ignore */ }
  }
  if (!state.node) albumOptions();
  showDraftNotice();
  if (me.level !== 2) $('#hint').textContent = me.level >= 4 ? '你是管理员，上传后直接公开。' : me.level >= 3 ? '你的上传要由另一位审核员通过后公开。' : '你的上传会在审核通过后公开。';
})();

function yearOptions(cur) {
  const y = new Date().getFullYear();
  const acad = Array.from({ length: 16 }, (_, i) => `${y - i}-${y - i + 1}`);
  const single = Array.from({ length: 16 }, (_, i) => `${y + 1 - i}`);
  const opt = (v) => `<option${v === cur ? ' selected' : ''}>${v}</option>`;
  // Exam months (四六级 202406) and older years aren't in the lists; keep them selectable.
  const extra = cur && !acad.includes(cur) && !single.includes(cur) ? opt(cur) : '';
  return `${extra}<optgroup label="学年">${acad.map(opt).join('')}</optgroup><optgroup label="年份">${single.map(opt).join('')}</optgroup>`;
}

// ---------------------------------------------------------------- node picker

/** The hidden 「待整理」 node (see the server's inbox.rs): no parent, fixed name. */
const isInbox = (node) => !!node && node.parent == null && node.name === '待整理';

function pick(node, path) {
  state.node = node;
  state.path = path || [];
  $('#nodepick').hidden = true;
  $('#coursenew').hidden = true;
  $('#picked').hidden = false;
  $('#picked').innerHTML = `<div><b>${esc(node.name)}</b> <span class="small muted">${esc(pathText(state.path))}</span>
    <div class="small faint">${isInbox(node)
      ? '审核员会把这些文件归到正确的课程、改好名字后再公开。备注里写一句是什么课、什么内容会更快。'
      : `文件名将以「${esc(node.label || node.name)}」开头`}</div></div><span class="grow"></span><a href="#" id="unpick">更换</a>`;
  $('#unpick').onclick = (e) => { e.preventDefault(); state.node = null; saveDraft(); $('#picked').hidden = true; $('#nodepick').hidden = false; $('#nq').focus(); albumOptions(); };
  saveDraft();
  renderRows();
  albumOptions();
}

// ---------------------------------------------------------------- 合集 (optional)
//
// The uploader can put this batch into a new 合集 (name, source, year) or add it to one the
// course already has. After the files are in, the 合集 is proposed with them, in file-name
// order; a reviewer approves it together with the files.

let albums = [];
async function albumOptions() {
  const sel = $('#al_pick');
  const n = state.node;
  albums = [];
  if (n && !isInbox(n)) {
    try { albums = (await api(`/nodes/${n.id}`)).series || []; } catch { /* no list: new only */ }
  }
  const keep = sel.value;
  sel.innerHTML = '<option value="">不放进合集</option>' + (n && !isInbox(n)
    ? `<option value="new">新建合集…</option>${albums.map((s) => `<option value="${s.id}">加到「${esc(s.title)}」（已有 ${s.items.length} 份）</option>`).join('')}` : '');
  sel.value = [...sel.options].some((o) => o.value === keep) ? keep : '';
  $('#al_tip').textContent = n && !isInbox(n) ? '' : '先在第 1 步选好课程，才能放进合集（一个合集里的资料要在同一门课）。';
  sel.disabled = !n || isInbox(n);
  $('#al_new').hidden = sel.value !== 'new';
}
$('#al_pick').onchange = () => { $('#al_new').hidden = $('#al_pick').value !== 'new'; if ($('#al_pick').value === 'new') $('#al_title').focus(); };

/** Checks the 合集 choice before anything is sent; null when it's fine. */
function albumProblem(todo) {
  const v = $('#al_pick').value;
  if (!v) return null;
  if (v === 'new' && !$('#al_title').value.trim()) return '请填写合集名称，或者选「不放进合集」';
  if (todo.some((r) => nodeOf(r)?.id !== state.node.id)) return `放进合集的文件要都在「${state.node.name}」里：有文件单独选了别的分类`;
  if (v === 'new' && todo.length < 2) return '合集至少要有两份资料';
  return null;
}

/** Proposes the 合集 with the files just uploaded. */
async function sendAlbum(done) {
  const v = $('#al_pick').value;
  if (!v || !done.length) return '';
  const ids = done.slice().sort((a, b) => a.file.name.localeCompare(b.file.name, 'zh-CN', { numeric: true })).map((r) => r.rid);
  try {
    let s;
    if (v === 'new') {
      if (ids.length < 2) return '<div class="notice warn">只传成功了一份，没有建合集；其他文件传好后可以在课程页勾选「整理成合集」。</div>';
      s = await api('/series', { method: 'POST', body: { node: state.node.id, title: $('#al_title').value, source: $('#al_source').value, year: $('#al_year').value, items: ids } });
    } else {
      const cur = await api(`/series/${v}`);
      s = await api(`/series/${v}`, { method: 'PUT', body: { title: cur.title, source: cur.source, year: cur.year, items: [...cur.items.map((i) => i.id), ...ids] } });
    }
    $('#al_pick').value = '';
    $('#al_new').hidden = true;
    return `<div class="notice ok">${s.draft ? `合集「${esc(s.draft.title)}」已提交，审核员会和资料一起审核。` : `合集「${esc(s.title)}」已更新。`}</div>`;
  } catch (err) {
    return `<div class="notice warn">资料已上传，但合集没建成：${esc(err.message)}。可以到<a href="/n/${state.node.id}">课程页</a>勾选这些资料再点「整理成合集」。</div>`;
  }
}

let timer = 0;
let seq = 0;
$('#nq').oninput = () => {
  clearTimeout(timer);
  timer = setTimeout(async () => {
    const q = $('#nq').value.trim();
    const ul = $('#ns');
    if (!q) { ul.hidden = true; return; }
    const my = ++seq;
    const list = await api(`/nodes/suggest?q=${encodeURIComponent(q)}`).catch(() => []);
    if (my !== seq) return;
    ul.hidden = false;
    ul.innerHTML = (list.length
      ? list.map((x, i) => `<li data-i="${i}">${esc(x.node.name)}<small>${esc(pathText(x.path))}${x.node.status === 'pending' ? ' · 待确认' : ''}</small></li>`).join('')
      : '<li class="faint">没有找到，试试拼音首字母，或在分类树里选</li>') +
      `<li data-new="1"><b>＋ 新增课程「${esc(q)}」</b><small>找不到就自己建一个，审核员会确认</small></li>`;
    ul.onclick = (e) => {
      if (e.target.closest('li[data-new]')) { ul.hidden = true; openNewCourse(q); return; }
      const li = e.target.closest('li[data-i]');
      if (!li) return;
      ul.hidden = true;
      const x = list[Number(li.dataset.i)];
      pick(x.node, x.path);
    };
  }, 160);
};
document.addEventListener('click', (e) => { if (!e.target.closest('.suggest')) $('#ns').hidden = true; });
// ↑ ↓ through the suggestions, Enter picks, Esc closes.
arrowKeys($('#nq'), $('#ns'), 'li[data-i], li[data-new]');
$('#nq').addEventListener('keydown', (e) => { if (e.key === 'Escape') $('#ns').hidden = true; });

/** New-course form: pick the category (校选课 / 公共课 / 专业课 / 体育课 …), then the offering
 * college or group, as on the course-selection site. */
async function openNewCourse(name = '') {
  const t = await tree();
  // Some existing college nodes were saved as courses directly under a section.
  // Their position still makes them offering groups for this form.
  const secs = t.children(0).filter((s) => t.children(s.id).some((g) => g.kind === 'group' || g.kind === 'course'));
  const ss = $('#nc_sec');
  const keep = ss.value;
  ss.innerHTML = secs.map((s) => `<option value="${s.id}">${esc(s.name)}</option>`).join('');
  // Default to 专业课 unless the user already chose.
  const b = secs.find((s) => s.code === 'B');
  ss.value = keep || (b ? String(b.id) : ss.value);
  const fill = () => {
    const sel = $('#nc_parent');
    const prev = sel.value;
    const groups = t.children(Number(ss.value)).filter((g) => g.kind === 'group' || g.kind === 'course');
    sel.innerHTML = groups.map((g) => `<option value="${g.id}">${esc(g.name)}</option>`).join('');
    if (groups.some((g) => String(g.id) === prev)) sel.value = prev;
    else {
      // Keep the same college when switching category (专业课 信息学院 → 校选课 信息学院).
      const was = t.byId.get(Number(prev))?.name;
      const same = was && groups.find((g) => g.name === was);
      if (same) sel.value = String(same.id);
    }
  };
  ss.onchange = fill;
  fill();
  if (name) $('#nc_name').value = name;
  $('#nodepick').hidden = true;
  $('#coursenew').hidden = false;
  $('#nc_name').focus();
}
$('#useinbox').onclick = async (e) => {
  e.preventDefault();
  try { pick(await api('/inbox', { method: 'POST' }), []); } catch (err) { toast(err.message, true); }
};
$('#newcourse').onclick = (e) => { e.preventDefault(); openNewCourse($('#nq').value.trim()); };
$('#backpick').onclick = (e) => { e.preventDefault(); $('#coursenew').hidden = true; $('#nodepick').hidden = false; };
$('#nc_go').onclick = async () => {
  try {
    const n = await api('/nodes', { method: 'POST', body: { parent: Number($('#nc_parent').value), kind: 'course', name: $('#nc_name').value, level: Number($('#nc_level').value) } });
    const d = await api(`/nodes/${n.id}`);
    pick(d.node, d.path);
  } catch (err) { toast(err.message, true); }
};

// ---------------------------------------------------------------- filename recognition

/** Guesses name parts from an original file name (rule-based 自动识别). */
function guess(name) {
  const stem = name.replace(/\.[^.]+$/, '');
  const ext = (/\.([^.]+)$/.exec(name)?.[1] || '').toLowerCase();
  const g = { year: '', term: '', type_word: '', paper: '', with_answer: false, extra: '' };
  let m;
  if ((m = /(20\d{2})\s*[-–~至]\s*(20\d{2})/.exec(stem))) g.year = `${m[1]}-${m[2]}`;
  else if ((m = /(?<!\d)(\d{2})\s*[-–]\s*(\d{2})(?!\d)/.exec(stem)) && Number(m[2]) === Number(m[1]) + 1) g.year = `20${m[1]}-20${m[2]}`;
  else if ((m = /(?<!\d)(20\d{2})(0[1-9]|1[0-2])(?!\d)/.exec(stem))) g.year = `${m[1]}${m[2]}`;
  else if ((m = /(?<!\d)(20\d{2})(?!\d)/.exec(stem))) g.year = m[1];
  if (/第一学期|秋季|秋/.test(stem)) g.term = '秋';
  else if (/第二学期|春季|春/.test(stem)) g.term = '春';
  else if (/暑期|暑假|夏季学期|暑/.test(stem)) g.term = '暑';
  if ((m = /([ABC])\s*卷/i.exec(stem))) g.paper = `${m[1].toUpperCase()}卷`;
  if (/答案|解答|解析|参考答案|评分标准|solution|sln|\bkey\b/i.test(stem)) g.with_answer = true;
  if ((m = /(\d{2,4})\s*题/.exec(stem))) g.extra = `${m[1]}题`;
  const rules = [
    [/期中/, '期中试卷'], [/期末/, '期末试卷'], [/小测|测验|quiz/i, '小测'], [/思考题/, '思考题'], [/题库|刷题|选择题/, '题库'],
    [/真题|试卷|试题|往年|卷|exam|final|midterm/i, '往年试卷'], [/单词/, '单词表'], [/提纲|大纲/, '提纲'], [/重点|复习|考点/, '重点'],
    [/笔记|note/i, '笔记'], [/实验|报告/, '实验报告'], [/讲义/, '讲义'], [/课件|slides?|lecture/i, '课件'], [/教材|课本|教科书|第.版|edition/i, '教材'],
    [/模板|template/i, '模板'],
  ];
  // "期中复习重点" is notes, not a paper: study-material words win unless the name says 试卷/考试.
  const isPaper = /试卷|试题|考试|真题|[ABC]\s*卷/i.test(stem);
  const notes = [[/思考题/, '思考题'], [/题库|刷题/, '题库'], [/单词/, '单词表'], [/提纲|大纲/, '提纲'], [/重点|考点|复习/, '重点'], [/笔记/, '笔记']];
  if (!isPaper) for (const [re, w] of notes) if (re.test(stem)) { g.type_word = w; break; }
  if (!g.type_word) for (const [re, w] of rules) if (re.test(stem)) { g.type_word = w; break; }
  if (!g.type_word) {
    if (['ppt', 'pptx', 'key'].includes(ext)) g.type_word = '课件';
    else if (['zip', 'rar', '7z'].includes(ext)) g.type_word = '合集';
    else if (['html', 'exe', 'py'].includes(ext)) g.type_word = '工具';
    else if (g.with_answer) g.type_word = '往年试卷';
    else g.type_word = '资料';
  }
  if (g.paper && g.type_word === '资料') g.type_word = '往年试卷';
  return g;
}

/**
 * The course a file name names, if any: the longest course name / label / alias found in it.
 * Checked against the files already on the site (2026-09): about a third of original names
 * name a course at all, and of those ~88% match where reviewers filed them; most misses are a
 * related course (数据库 / 数据库系统, 概率统计 / 概率统计(A)). So the guess files a batch
 * nobody categorised, but only suggests against a category the uploader picked.
 */
function courseIn(name, t) {
  const stem = name.replace(/\.[^.]+$/, '').toLowerCase();
  let best = null;
  let len = 1; // single characters match too much
  for (const n of t.list) {
    if (n.kind !== 'course' || n.status !== 'active' || GENERIC.test(n.name)) continue;
    for (const w of new Set([n.name, n.label, ...(n.aliases || [])])) {
      if (!w || w.length < len) continue;
      const at = stem.indexOf(w.toLowerCase());
      if (at < 0) continue;
      // 「微积分I」 is not in 「微积分III」: a name ending in a Roman numeral may not run on
      // into another one (「线代」 still matches 「线代I」).
      if (/[ivxⅠ-ⅿ]$/i.test(w) && /^[ivxⅠ-ⅿ]/i.test(stem.slice(at + w.length))) continue;
      if (w.length === len && best) continue;
      best = n;
      len = w.length;
    }
  }
  return best;
}
/** Course names too general to file by (a 「教材」 course; 「未分层（微积分）」 placeholders). */
const GENERIC = /^(教材|资料|课件|笔记|未分层)/;

/** Courses sharing `c`'s name in other colleges (电工技术 is taught by two); which one a file
 * belongs to can't be told from its name. */
const sameName = (a, b) => a.name.toLowerCase() === b.name.toLowerCase();
const twinsOf = (c, t) => t.list.filter((n) => n.kind === 'course' && n.status === 'active' && n.id !== c.id && sameName(n, c));
/** The course a node lies in (itself, or the course above a level). */
function courseOf(t, id) {
  for (let n = t.byId.get(id); n; n = t.byId.get(n.parent)) if (n.kind === 'course') return n;
  return null;
}

/** Whether node `id` is `anc` or lies under it. */
function within(t, id, anc) {
  for (let n = t.byId.get(id); n; n = t.byId.get(n.parent)) if (n.id === anc) return true;
  return false;
}

function timeOf(r) {
  if (!r.year) return '';
  return r.term && /^\d{4}(-\d{4})?$/.test(r.year) ? `${r.year}${r.term}` : r.year;
}

/** The category a file goes to: its own, else the batch's. */
const nodeOf = (r) => r.node || state.node;
/** What the draft keeps of a node (enough to show it and name files). */
const brief = (n) => ({ id: n.id, name: n.name, label: n.label, parent: n.parent ?? null });

function genName(r) {
  const n = nodeOf(r);
  if (!n) return '（没选分类：放进「待整理」，审核员会归到正确的课程、改好名字）';
  let s = n.label || n.name;
  const t = timeOf(r);
  if (t) s += `_${t}`;
  s += `_${r.type_word}`;
  const d = [r.paper, r.with_answer && '含答案', r.extra].filter(Boolean);
  if (d.length) s += `(${d.join('、')})`;
  const ext = (/\.([^.]+)$/.exec(r.file.name)?.[1] || '').toLowerCase();
  return ext ? `${s}.${ext}` : s;
}

// ---------------------------------------------------------------- rows

async function addFiles(files) {
  const t = await tree().catch(() => null);
  for (const f of files) {
    if (!f.size) { toast(`${f.name} 是空文件，已跳过`, true); continue; }
    if (f.size > M.limits.max_file) { toast(`${f.name} 太大了`, true); continue; }
    if (state.rows.some((r) => r.file.name === f.name && r.file.size === f.size)) continue;
    const g = guess(f.name);
    const row = { file: f, sel: true, parts: null, hashing: 0, status: '', err: false, done: false, note: '', subtitle: '', ...g };
    const saved = draft.files && draft.files[fileKey(f)];
    if (saved) {
      for (const k of FIELDS) if (k in saved) row[k] = saved[k];
      if (Array.isArray(saved.parts)) row.parts = saved.parts;
      if (saved.upload_id) row.upload_id = saved.upload_id;
      row.status = saved.upload_id ? '已恢复上次填写的信息，上传会从断点继续' : '已恢复上次填写的信息';
    } else if (t) {
      // A file named after a course goes there when no category was picked; against one the
      // uploader picked it is only a suggestion (see courseIn).
      const c = courseIn(f.name, t);
      const twins = c ? twinsOf(c, t) : [];
      const mine = state.node && courseOf(t, state.node.id);
      if (!c || (mine && sameName(mine, c))) { /* nothing to add */ }
      else if (twins.length) {
        // Several colleges teach a course of this name: don't guess which.
        if (!state.node) row.status = `「${esc(c.name)}」有 ${twins.length + 1} 门同名课程（不同学院），请点「单独选分类」选一下；不选就放进「待整理」`;
      } else if (!state.node) {
        row.node = brief(c);
        row.status = `按文件名放到了「${esc(c.name)}」，不对的话点「换一个」`;
      } else if (!within(t, state.node.id, c.id)) row.guess = brief(c);
    }
    state.rows.push(row);
  }
  saveDraft();
  showDraftNotice();
  renderRows();
  hashQueue();
}

function rowHtml(r, i) {
  const types = M.type_words.map((t) => `<option${t.word === r.type_word ? ' selected' : ''}>${t.word}</option>`).join('');
  const hashed = r.parts ? '' : ` · 校验中 ${Math.round(r.hashing * 100)}%`;
  return `<div class="frow${r.done ? ' done' : r.err ? ' err' : ''}" data-i="${i}">
    <div class="head"><input type="checkbox" data-f="sel"${r.sel ? ' checked' : ''}${r.done ? ' disabled' : ''}>
      <span class="orig">${esc(r.file.name)} <span class="faint">· ${fmtSize(r.file.size)}${hashed}</span></span>
      ${r.done ? '' : `<button class="btn sm" data-x="del" type="button">移除</button>`}</div>
    <div class="gen">→ ${esc(genName(r))}</div>
    ${r.done ? '' : `<div class="small fnode">分类：${r.node ? `<b>${esc(r.node.name)}</b>（这个文件单独设置） · <a href="#" data-x="unnode">跟随上面的分类</a> · ` : `<span class="faint">${state.node ? esc(state.node.name) : '跟随上面的分类'}</span> · `}<a href="#" data-x="node">${r.node ? '换一个' : '单独选分类'}</a>${r.guess && !r.node ? ` · <span class="guess">文件名像是「${esc(r.guess.name)}」的，<a href="#" data-x="guess">放过去</a></span>` : ''}</div>`}
    <div class="opts">
      <label>类型<select class="input" data-f="type_word">${types}</select></label>
      <label>学年 / 年份<select class="input" data-f="year"><option value="">不填</option>${yearOptions(r.year)}</select></label>
      <label>学期<select class="input" data-f="term"><option value="">不填</option>${['秋', '春', '暑'].map((t) => `<option${t === r.term ? ' selected' : ''}>${t}</option>`).join('')}</select></label>
      <label>卷别<select class="input" data-f="paper"><option value="">无</option>${M.papers.map((p) => `<option${p === r.paper ? ' selected' : ''}>${p}</option>`).join('')}</select></label>
      <label>补充（如 201题）<input class="input" data-f="extra" value="${esc(r.extra)}" maxlength="20"></label>
      <label title="同一门课不同专业考的不一样时填">适用专业（选填）<input class="input" data-f="major" value="${esc(r.major || '')}" maxlength="20" placeholder="如 软件工程"></label>
      <label style="flex-direction:row;align-items:center;gap:6px;padding-bottom:8px"><input type="checkbox" data-f="with_answer"${r.with_answer ? ' checked' : ''}> 含答案</label>
      <label class="wide" style="grid-column:1/-1">小标题（选填，公开显示，说明具体内容；留空则在原文件名能看懂时自动使用它）<input class="input" data-f="subtitle" value="${esc(r.subtitle || '')}" maxlength="80" placeholder="${esc(r.file.name.replace(/\.[^.]+$/, ''))}"></label>
      <label class="wide" style="grid-column:1/-1">备注（选填，公开显示，如“只有选择题答案”）<input class="input" data-f="note" value="${esc(r.note)}" maxlength="500"></label>
    </div>
    ${r.status ? `<div class="st ${r.err ? 'bad' : ''}" style="color:${r.err ? 'var(--bad)' : r.done ? 'var(--ok)' : 'var(--muted)'}">${r.status}</div>` : ''}
  </div>`;
}

function renderRows() {
  const guesses = state.rows.filter((r) => r.guess && !r.node && !r.done).length;
  $('#rows').innerHTML = (guesses ? `<div class="notice warn small" style="margin-bottom:10px">有 ${guesses} 个文件的文件名看起来属于别的课程（下面标黄的）。<a href="#" data-x="guessall">全部按文件名放过去</a>，或者逐个看一下。</div>` : '')
    + state.rows.map(rowHtml).join('');
  $('#bulk').hidden = state.rows.length < 2;
  const pending = state.rows.filter((r) => !r.done).length;
  $('#submit').textContent = pending > 1 ? `上传并提交 ${pending} 个文件` : '上传并提交';
}

function updateRow(i) {
  const el = document.querySelector(`.frow[data-i="${i}"]`);
  if (el) el.outerHTML = rowHtml(state.rows[i], i);
}

$('#rows').addEventListener('change', (e) => {
  const row = e.target.closest('.frow');
  const f = e.target.dataset.f;
  if (!row || !f) return;
  const r = state.rows[Number(row.dataset.i)];
  r[f] = e.target.type === 'checkbox' ? e.target.checked : e.target.value;
  saveDraft();
  if (f !== 'note' && f !== 'sel' && f !== 'subtitle') row.querySelector('.gen').textContent = `→ ${genName(r)}`;
});
$('#rows').addEventListener('input', (e) => {
  const row = e.target.closest('.frow');
  const f = e.target.dataset.f;
  if (!row || (f !== 'extra' && f !== 'note' && f !== 'subtitle')) return;
  const r = state.rows[Number(row.dataset.i)];
  r[f] = e.target.value;
  saveDraft();
  if (f === 'extra') row.querySelector('.gen').textContent = `→ ${genName(r)}`;
});
$('#rows').addEventListener('click', async (e) => {
  const g = e.target.closest('[data-x="guess"], [data-x="guessall"]');
  if (g && !state.busy) {
    e.preventDefault();
    const rows = g.dataset.x === 'guessall' ? state.rows : [state.rows[Number(g.closest('.frow').dataset.i)]];
    for (const r of rows) if (r.guess && !r.node && !r.done) r.node = r.guess;
    saveDraft();
    renderRows();
    return;
  }
  const nb = e.target.closest('[data-x="node"], [data-x="unnode"]');
  if (nb && !state.busy) {
    e.preventDefault();
    const i = Number(nb.closest('.frow').dataset.i);
    const r = state.rows[i];
    if (nb.dataset.x === 'unnode') r.node = null;
    else {
      const x = await pickFromTree(`「${r.file.name}」放到哪个分类`);
      if (!x) return;
      r.node = brief(x.node);
    }
    saveDraft();
    updateRow(i);
    return;
  }
  const b = e.target.closest('[data-x="del"]');
  if (!b || state.busy) return;
  const [gone] = state.rows.splice(Number(b.closest('.frow').dataset.i), 1);
  if (draft.files) delete draft.files[fileKey(gone.file)];
  saveDraft();
  renderRows();
});

$('#b_node').onclick = async () => {
  const sel = state.rows.filter((r) => r.sel && !r.done);
  if (!sel.length) return toast('先勾选文件', true);
  const x = await pickFromTree(`把选中的 ${sel.length} 个文件放到哪个分类`);
  if (!x) return;
  for (const r of sel) r.node = brief(x.node);
  saveDraft();
  renderRows();
};
$('#browsepick').onclick = async (e) => {
  e.preventDefault();
  const x = await pickFromTree('选择上传到哪个分类');
  if (x) pick(x.node, x.path);
};
$('#b_apply').onclick = () => {
  const v = { type_word: $('#b_type').value, year: $('#b_year').value, term: $('#b_term').value, paper: $('#b_paper').value, major: $('#b_major').value.trim() };
  for (const r of state.rows) {
    if (!r.sel || r.done) continue;
    for (const [k, x] of Object.entries(v)) if (x) r[k] = x;
    if ($('#b_ans').checked) r.with_answer = true;
  }
  saveDraft();
  renderRows();
};
$('#b_all').onchange = (e) => { for (const r of state.rows) if (!r.done) r.sel = e.target.checked; renderRows(); };

$('#file').onchange = (e) => { addFiles([...e.target.files]); e.target.value = ''; };
const drop = $('#drop');
drop.ondragover = (e) => { e.preventDefault(); drop.classList.add('over'); };
drop.ondragleave = () => drop.classList.remove('over');
drop.ondrop = (e) => { e.preventDefault(); drop.classList.remove('over'); addFiles([...e.dataTransfer.files]); };

// ---------------------------------------------------------------- hashing (one file at a time)

let hasher = null;
function loadHasher() {
  if (!hasher) {
    hasher = new Promise((resolve, reject) => {
      const s = document.createElement('script');
      s.src = '/vendor/sha256.umd.min.js';
      s.onload = () => resolve(window.hashwasm);
      s.onerror = () => reject(new Error('加载校验组件失败'));
      document.head.appendChild(s);
    });
  }
  return hasher;
}

let hashing = false;
async function hashQueue() {
  if (hashing) return;
  hashing = true;
  try {
    const hw = await loadHasher();
    for (;;) {
      const i = state.rows.findIndex((r) => !r.parts);
      if (i < 0) break;
      const r = state.rows[i];
      const f = r.file;
      const parts = [];
      const CHUNK = 4 * 1024 * 1024;
      let done = 0;
      let lastPaint = 0;
      for (let start = 0; start < f.size; start += M.limits.max_part) {
        const end = Math.min(f.size, start + M.limits.max_part);
        const h = await hw.createSHA256();
        h.init();
        for (let off = start; off < end; off += CHUNK) {
          h.update(new Uint8Array(await f.slice(off, Math.min(end, off + CHUNK)).arrayBuffer()));
          done = Math.min(f.size, off + CHUNK);
          r.hashing = done / f.size;
          if (performance.now() - lastPaint > 300) { lastPaint = performance.now(); updateRow(state.rows.indexOf(r)); }
        }
        parts.push({ start, end, size: end - start, sha256: h.digest('hex') });
      }
      r.parts = parts;
      saveDraft();
      updateRow(state.rows.indexOf(r));
    }
  } catch (e) {
    toast(e.message, true);
  } finally {
    hashing = false;
  }
}

// ---------------------------------------------------------------- upload

async function uploadRow(r, bar, base, total) {
  while (!r.parts) await new Promise((res) => setTimeout(res, 300));
  const f = r.file;
  r.status = '正在准备…';
  updateRow(state.rows.indexOf(r));
  // Resume the upload from before a refresh when the server still has it.
  let plan = null;
  if (r.upload_id) {
    plan = await api(`/uploads/${r.upload_id}`).catch(() => null);
    if (!plan || plan.parts.length !== r.parts.length) plan = null;
  }
  if (!plan) {
    plan = await api('/uploads', { method: 'POST', body: { filename: f.name, mime: f.type, parts: r.parts.map((p) => ({ size: p.size, sha256: p.sha256 })) } });
  }
  r.upload_id = plan.upload_id;
  saveDraft();
  if (!plan.dedup) {
    let sent = 0;
    for (const pp of plan.parts) {
      if (pp.done) { sent += pp.size; continue; }
      const part = r.parts[pp.index];
      let target = pp.target;
      for (let attempt = 0; ; attempt++) {
        try {
          r.status = plan.parts.length > 1 ? `上传第 ${pp.index + 1} / ${plan.parts.length} 卷…` : '上传中…';
          updateRow(state.rows.indexOf(r));
          if (attempt > 0) target = await renewTarget(plan.upload_id, pp.index, target);
          const receipt = await sendPart(target, f.slice(part.start, part.end), (n) => {
            bar.style.width = `${Math.round(((base + sent + n) / total) * 100)}%`;
          });
          await api(`/uploads/${plan.upload_id}/parts/${pp.index}`, { method: 'POST', body: { asset_id: receipt.id ?? null } });
          break;
        } catch (err) {
          if (attempt >= 2) throw err;
          r.status = `${esc(err.message)}，重试中…`;
          updateRow(state.rows.indexOf(r));
          await new Promise((res) => setTimeout(res, 1500 * (attempt + 1)));
        }
      }
      sent += pp.size;
    }
  }
  const body = {
    upload_id: plan.upload_id, node: nodeOf(r).id, time: timeOf(r), type_word: r.type_word,
    paper: r.paper, with_answer: r.with_answer, extra: r.extra, note: r.note,
    ...(r.subtitle && r.subtitle.trim() ? { subtitle: r.subtitle } : {}),
    ...(r.major && r.major.trim() ? { major: r.major.trim() } : {}),
  };
  let res;
  try {
    res = await api('/resources', { method: 'POST', body });
  } catch (err) {
    // A resumed upload that the server has expired or already used: start it over once.
    if (err.status === 404 || err.status === 409) { r.upload_id = null; saveDraft(); throw new Error(`${err.message}，请再点一次上传重试`); }
    throw err;
  }
  r.done = true;
  r.rid = res.id;
  r.sel = false;
  r.upload_id = null;
  if (draft.files) delete draft.files[fileKey(r.file)];
  saveDraft();
  r.status = res.status === 'published'
    ? `✓ 已发布：<a href="/r/${res.id}">${esc(res.filename)}</a>`
    : `✓ 已提交，等待审核：<a href="/r/${res.id}">${esc(res.filename)}</a>`;
}

/** A fresh slot for a part whose send failed. After a failure on the upload Worker (another
 * site; unreachable on some networks, or out of its daily quota) the retry goes through this
 * server's relay instead. */
async function renewTarget(uploadId, index, failed) {
  const viaRelay = failed && !failed.url.startsWith('/');
  return (await api(`/uploads/${uploadId}/parts/${index}/renew${viaRelay ? '?via=relay' : ''}`, { method: 'POST' })).target;
}

/** Files following the picked category whose names say another course (checked again at submit:
 * the category may have been picked after the files were added). */
async function mismatches(todo) {
  const t = await tree().catch(() => null);
  const mine = t && state.node && courseOf(t, state.node.id);
  if (!t || !state.node) return { follow: 0, off: [] };
  const follow = todo.filter((r) => !r.node);
  const off = [];
  for (const r of follow) {
    const c = courseIn(r.file.name, t);
    if (!c || (mine && sameName(mine, c)) || within(t, state.node.id, c.id)) continue;
    off.push({ r, c, twins: twinsOf(c, t).length > 0 });
  }
  return { follow: follow.length, off };
}

/** Half or more of the files look like another course than the picked one: ask before sending
 * them all there (a whole batch once went under 微积分I this way). Resolves true to go on. */
async function confirmCategory(todo) {
  const { follow, off } = await mismatches(todo);
  if (!off.length || off.length * 2 < follow) return true;
  const names = [...new Set(off.map((x) => x.c.name))];
  const movable = off.filter((x) => !x.twins);
  return new Promise((resolve) => {
    const body = modal('分类好像不对');
    let answered = false;
    const done = (v) => { answered = true; body.close(); resolve(v); };
    body.innerHTML = `<p>你选的分类是「<b>${esc(state.node.name)}</b>」，但这 ${follow} 个文件里有 <b>${off.length}</b> 个的文件名看起来属于别的课程：${names.slice(0, 5).map((n) => `「${esc(n)}」`).join('')}${names.length > 5 ? ` 等 ${names.length} 门` : ''}。</p>
      <p class="small muted">放错课程的资料别人很难找到，审核员也要一份份挪。${off.length > movable.length ? '有些课程名在几个学院都有，按文件名没法确定是哪个，请逐个选一下。' : ''}</p>
      <div class="row" style="margin-top:12px;flex-wrap:wrap">
        ${movable.length ? `<button class="btn primary sm" data-a="move" type="button">按文件名放过去（${movable.length} 个）</button>` : ''}
        <button class="btn sm" data-a="back" type="button">回去逐个检查</button>
        <button class="btn sm" data-a="go" type="button">确定都放到「${esc(state.node.name)}」</button></div>`;
    body.onclick = (e) => {
      const a = e.target.closest('[data-a]')?.dataset.a;
      if (!a) return;
      if (a === 'move') {
        for (const x of movable) x.r.node = brief(x.c);
        saveDraft();
        renderRows();
        toast(`已把 ${movable.length} 个文件放到文件名对应的课程，检查一下再点上传`);
      }
      done(a === 'go');
    };
    // Closed with × or Esc: don't upload.
    const obs = new MutationObserver(() => { if (!body.isConnected) { obs.disconnect(); if (!answered) resolve(false); } });
    obs.observe(document.body, { childList: true });
  });
}

$('#submit').onclick = async () => {
  const todo = state.rows.filter((r) => !r.done);
  if (!todo.length) return toast('请先添加文件', true);
  if (!$('#agree').checked) { $('#agree').scrollIntoView({ behavior: 'smooth', block: 'center' }); return toast('请先勾选确认你有权分享这些资料', true); }
  if (!(await confirmCategory(todo))) return;
  const albumErr = albumProblem(todo);
  if (albumErr) { $('#albumcard').scrollIntoView({ behavior: 'smooth', block: 'center' }); return toast(albumErr, true); }
  // Files with no category (none picked, none recognised from the name) go to 「待整理」.
  if (todo.some((r) => !nodeOf(r))) {
    try {
      const inbox = brief(await api('/inbox', { method: 'POST' }));
      for (const r of todo) if (!nodeOf(r)) r.node = inbox;
      saveDraft();
    } catch (err) { return toast(err.message, true); }
  }
  state.busy = true;
  $('#submit').disabled = true;
  $('#prog').hidden = false;
  const bar = $('#prog').firstElementChild;
  const total = todo.reduce((s, r) => s + r.file.size, 0);
  let base = 0;
  let ok = 0;
  for (const r of todo) {
    r.err = false;
    try {
      await uploadRow(r, bar, base, total);
      ok++;
    } catch (err) {
      r.err = true;
      r.status = esc(err.message);
    }
    base += r.file.size;
    updateRow(state.rows.indexOf(r));
  }
  bar.style.width = '100%';
  state.busy = false;
  $('#submit').disabled = false;
  renderRows();
  const album = await sendAlbum(todo.filter((r) => r.done && r.rid));
  $('#status').innerHTML = `<div class="notice ${ok === todo.length ? 'ok' : 'warn'}">完成 ${ok} / ${todo.length} 个文件${ok < todo.length ? '，失败的可以直接再点一次上传重试' : '，谢谢你的分享！'}</div>${album}`;
};
