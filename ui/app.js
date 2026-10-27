const { invoke } = window.__TAURI__.core;

const $ = (sel) => document.querySelector(sel);

const state = {
  view: null,
  tab: 'goals',
  filter: 'All',
  kind: 'counter',
  archiveOpen: false,
  // Last rendered width of each bar, so re-renders animate from the old value.
  shown: new Map(),
};
let lastClick = { x: innerWidth / 2, y: innerHeight / 2 };

// ---------- Formatting ----------

const esc = (s) =>
  String(s).replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]);
const fmt = (n) => (+(+n).toFixed(2)).toLocaleString();
const fmt1 = (n) => (+(+n).toFixed(1)).toLocaleString();
const pct = (p) => Math.floor(p * 100);
const pad = (x) => String(x).padStart(2, '0');

function fmtTime(minutes) {
  let s = Math.round(minutes * 60);
  const h = Math.floor(s / 3600);
  s -= h * 3600;
  const m = Math.floor(s / 60);
  return h ? `${h}:${pad(m)}:${pad(s % 60)}` : `${m}:${pad(s % 60)}`;
}

/** "20" → 20 min, "20:30" → 20.5 min, "1:05:30" → 65.5 min. */
function parseTime(text) {
  const parts = String(text).trim().split(':').map(Number);
  if (parts.some((x) => !Number.isFinite(x) || x < 0)) return NaN;
  const total = parts.reduce((acc, x) => acc * 60 + x, 0);
  return parts.length === 1 ? total : total / 60;
}

const pace = (minutes, km) => `${fmtTime(minutes / km)}/km`;

function hue(text) {
  let h = 0;
  for (const c of text) h = (h * 31 + c.codePointAt(0)) % 360;
  return h;
}

// ---------- Backend ----------

async function call(cmd, args = {}) {
  try {
    const res = await invoke(cmd, args);
    state.view = res.view;
    render();
    playEvents(res.events);
    return true;
  } catch (err) {
    toast(`⚠️ ${esc(err)}`, 'error');
    return false;
  }
}

// ---------- Rendering ----------

function render() {
  const v = state.view;
  if (!v) return;
  renderPlayer(v.profile);
  document.querySelectorAll('.tab').forEach((t) => t.classList.toggle('active', t.dataset.tab === state.tab));
  $('#quest-count').textContent = v.quests.quests.filter((q) => !q.completed).length || '';
  const pages = { goals: goalsHtml, quests: questsHtml, trophies: trophiesHtml };
  $('#main').innerHTML = pages[state.tab](v);
  animateBars();
}

function animateBars() {
  const fills = document.querySelectorAll('.fill[data-key]');
  fills.forEach((el) => (el.style.width = `${(state.shown.get(el.dataset.key) ?? 0) * 100}%`));
  // Two frames so the browser commits the old width before transitioning.
  requestAnimationFrame(() =>
    requestAnimationFrame(() =>
      fills.forEach((el) => {
        const target = Math.max(0, Math.min(1, +el.dataset.target));
        el.style.width = `${target * 100}%`;
        state.shown.set(el.dataset.key, target);
      }),
    ),
  );
}

function renderPlayer(p) {
  const span = p.next_level_xp - p.level_start_xp;
  const into = p.xp - p.level_start_xp;
  const vsGhost = p.xp_this_week - p.ghost_xp_by_now;
  $('#player').innerHTML = `
    <div class="level-badge" title="Level ${p.level} · ${fmt(p.xp)} XP total"><span>${p.level}</span></div>
    <div class="xp-block">
      <div class="xp-top"><strong>${esc(p.title)}</strong><span class="muted">${fmt(into)} / ${fmt(span)} XP</span></div>
      <div class="bar xp"><div class="fill" data-key="xp" data-target="${into / span}"></div></div>
    </div>
    <div class="stat" title="Days in a row with progress · best ${p.best_streak}">
      <span class="flame ${p.streak ? 'lit' : ''}">🔥</span><b>${p.streak}</b><small>day streak</small>
    </div>
    <div class="stat" title="XP this week (${fmt(p.xp_this_week)}) vs. your ghost: last week at this time (${fmt(p.ghost_xp_by_now)}). Last week total: ${fmt(p.ghost_xp_last_week)}">
      <span>👻</span><b class="${vsGhost >= 0 ? 'good' : 'bad'}">${vsGhost >= 0 ? '+' : ''}${fmt(vsGhost)}</b><small>XP vs ghost</small>
    </div>`;
}

function goalsHtml(v) {
  const cats = ['All', ...v.categories.filter((c) => v.goals.some((g) => g.category === c))];
  if (!cats.includes(state.filter)) state.filter = 'All';
  const goals = v.goals.filter((g) => state.filter === 'All' || g.category === state.filter);
  const chips = cats
    .map(
      (c) =>
        `<button class="chip-btn ${c === state.filter ? 'on' : ''}" data-action="filter" data-cat="${esc(c)}"
          ${c === 'All' ? '' : `style="--hue:${hue(c)}"`}>${esc(c)}</button>`,
    )
    .join('');
  const body = goals.length
    ? `<div class="grid">${goals.map(goalCard).join('')}</div>`
    : `<div class="empty">
        <div class="empty-icon">${v.trophies.length ? '🏆' : '🎯'}</div>
        <h2>${v.trophies.length ? 'Every goal conquered!' : 'No goals yet'}</h2>
        <p class="muted">${v.trophies.length ? 'Time to set a new challenge.' : 'Create your first goal and start earning XP.'}</p>
        <button class="btn primary" data-action="new-goal">＋ New goal</button>
      </div>`;
  return `<div class="toolbar"><div class="chips">${chips}</div>
    <button class="btn primary" data-action="new-goal">＋ New goal</button></div>${body}${archiveHtml(v)}`;
}

function archiveHtml(v) {
  if (!v.archived.length) return '';
  const rows = state.archiveOpen ? `<div class="archive-list">${v.archived.map(archivedRow).join('')}</div>` : '';
  return `<h2 class="section-title"><button class="archive-toggle" data-action="toggle-archive">
      ${state.archiveOpen ? '▾' : '▸'} Archive <span class="muted">${v.archived.length}</span></button></h2>${rows}`;
}

function archivedRow(g) {
  const status = g.completed_at
    ? `🏆 Completed ${new Date(g.completed_at).toLocaleDateString()}`
    : `Incomplete · ${pct(g.progress)}%`;
  const xp = g.xp_earned ? ` · ${fmt(g.xp_earned)} XP earned` : '';
  return `<article class="card archived">
    <div><h3>${esc(g.name)}</h3><p class="muted small">${esc(g.category)} · ${status}${xp}</p></div>
    <div class="icon-btns">
      <button class="btn small" data-action="restore" data-id="${g.id}">Restore</button>${deleteButton(g)}
    </div>
  </article>`;
}

function goalButtons(g) {
  const undo =
    g.undo_xp == null
      ? ''
      : `<button class="icon-btn" data-action="undo" data-id="${g.id}" data-xp="${g.undo_xp}"
          title="Undo last update${g.undo_xp ? ` (−${fmt(g.undo_xp)} XP)` : ''}">↶</button>`;
  return `<div class="icon-btns">${undo}
    <button class="icon-btn" data-action="archive" data-id="${g.id}" title="Archive: hide this goal and keep its XP">🗄</button>
    ${deleteButton(g)}</div>`;
}

const deleteButton = (g) =>
  `<button class="icon-btn" data-action="delete" data-id="${g.id}" data-xp="${g.xp_earned}"
    title="Delete goal${g.xp_earned ? ` and lose ${fmt(g.xp_earned)} XP` : ''}">✕</button>`;

function goalCard(g) {
  const perf = g.kind === 'performance';
  const status = perf
    ? g.best_run
      ? `Best: ${fmt(g.best_run.distance_km)} km in ${fmtTime(g.best_run.time_min)}`
      : 'No runs logged yet'
    : `${fmt(g.current)} / ${fmt(g.target)} ${esc(g.unit)}`;
  const targetLine = perf
    ? `<div class="muted small">Target: ${fmt(g.target_distance_km)} km in ${fmtTime(g.target_time_min)} (${pace(g.target_time_min, g.target_distance_km)})${
        g.best_run ? ` · your best pace ${pace(g.best_run.time_min, g.best_run.distance_km)}` : ''
      }</div>`
    : '';
  const rival = g.rival
    ? `<div class="rival" style="left:${g.rival.expected * 100}%" title="Pace rival: where you should be today to hit the deadline"></div>`
    : '';
  return `<article class="card goal" style="--hue:${hue(g.category)}">
    <div class="goal-head">
      <span class="chip">${esc(g.category)}</span>
      ${goalButtons(g)}
    </div>
    <h3>${esc(g.name)}</h3>
    <div class="goal-stats"><span class="big">${pct(g.progress)}<small>%</small></span><span class="muted">${status}</span></div>
    <div class="bar-wrap"><div class="bar goal-bar"><div class="fill" data-key="g${g.id}" data-target="${g.progress}"></div></div>${rival}</div>
    ${targetLine}
    <div class="tags">${rivalTag(g)}${ghostTag(g)}</div>
    ${perf ? runActions(g) : counterActions(g)}
  </article>`;
}

function rivalTag(g) {
  const r = g.rival;
  if (!r) return '';
  const amount = g.kind === 'performance' ? `${Math.abs(Math.round(r.ahead))} pts` : `${fmt1(Math.abs(r.ahead))} ${esc(g.unit)}`;
  const when = r.days_left < 0 ? `${-r.days_left}d overdue` : r.days_left === 0 ? 'due today' : `${r.days_left}d left`;
  const need = r.per_day_needed > 0 ? ` · ${fmt1(r.per_day_needed)}/day` : '';
  const ahead = r.ahead >= 0;
  return `<span class="tag ${ahead ? 'good' : 'bad'}" title="Pace rival">🏁 ${ahead ? 'Ahead by' : 'Behind by'} ${amount} · ${when}${need}</span>`;
}

function ghostTag(g) {
  const { this_week, by_now, last_week } = g.ghost;
  const show = g.kind === 'performance' ? (x) => `${pct(x)}%` : fmt1;
  if (!this_week && !by_now && !last_week) {
    return `<span class="tag" title="Next week you'll race against this week">👻 Set the pace for your ghost</span>`;
  }
  const winning = this_week >= by_now;
  const label = g.kind === 'performance' ? 'best run' : esc(g.unit);
  return `<span class="tag ${winning ? 'good' : 'warn'}" title="This week vs. last week at this time. Last week total: ${show(last_week)}">
    👻 ${show(this_week)} vs ${show(by_now)} ${label}${winning ? ' ✓' : ''}</span>`;
}

function counterActions(g) {
  return `<div class="actions">
    <button class="btn small" data-action="add" data-id="${g.id}" data-delta="-1" title="Subtract one">−1</button>
    <button class="btn small accent" data-action="add" data-id="${g.id}" data-delta="1">+1</button>
    <button class="btn small accent" data-action="add" data-id="${g.id}" data-delta="5">+5</button>
    <input class="amount" type="number" step="any" placeholder="Amount" />
    <button class="btn small" data-action="add-custom" data-id="${g.id}" title="Add this amount">Add</button>
    <button class="btn small ghost" data-action="set-total" data-id="${g.id}" title="Set the total to this value">Set total</button>
  </div>`;
}

function runActions(g) {
  return `<div class="actions">
    <input class="km" type="number" step="any" min="0" placeholder="km" />
    <input class="time" placeholder="mm:ss" />
    <button class="btn small accent" data-action="log-run" data-id="${g.id}">🏃 Log run</button>
  </div>`;
}

function questsHtml(v) {
  const q = v.quests;
  const ms = Math.max(0, new Date(q.ends_at) - Date.now());
  const days = Math.floor(ms / 86400000);
  const hours = Math.floor((ms % 86400000) / 3600000);
  const icons = { add_to_goal: '🎯', log_runs: '🏃', active_days: '📅' };
  const cards = q.quests
    .map(
      (x, i) => `<article class="card quest ${x.completed ? 'done' : ''}">
        <div class="quest-icon">${x.completed ? '✅' : icons[x.kind]}</div>
        <div class="quest-body">
          <h3>${esc(x.title)}</h3>
          <div class="bar"><div class="fill" data-key="q${q.week}-${i}" data-target="${Math.min(1, x.progress / x.target)}"></div></div>
          <span class="muted small">${fmt(Math.min(x.progress, x.target))} / ${fmt(x.target)}</span>
        </div>
        <div class="reward">+${x.xp}<small>XP</small></div>
      </article>`,
    )
    .join('');
  return `<div class="section-head">
      <h2>Weekly quests · Week ${esc(q.week.split('-W')[1])}</h2>
      <p class="muted">New quests in ${days}d ${hours}h. Targets are set to beat last week's ghost. Clear them all for the Clean Sweep badge.</p>
    </div>
    <div class="quest-list">${cards || '<p class="muted">Create a goal to unlock quests.</p>'}</div>`;
}

function trophiesHtml(v) {
  const p = v.profile;
  const unlocked = v.badges.filter((b) => b.unlocked_at).length;
  const stats = [
    ['🏆', v.trophies.length, 'goals completed'],
    ['⭐', fmt(p.xp), 'total XP'],
    ['🔥', p.best_streak, 'best streak'],
    ['🗺️', p.quests_completed, 'quests done'],
  ]
    .map(([icon, value, label]) => `<div class="card stat-card"><span class="icon">${icon}</span><div><b>${value}</b><span class="muted">${label}</span></div></div>`)
    .join('');
  const trophies = v.trophies.length
    ? v.trophies
        .map((g) => {
          const result =
            g.kind === 'performance' && g.best_run
              ? `${fmt(g.best_run.distance_km)} km in ${fmtTime(g.best_run.time_min)}`
              : `${fmt(g.current)} ${esc(g.unit)}`;
          const took = g.days_taken === 0 ? 'same day' : `took ${g.days_taken} day${g.days_taken === 1 ? '' : 's'}`;
          return `<article class="card trophy">
            <div class="trophy-icon">🏆</div>
            <div>
              <h3>${esc(g.name)}</h3>
              <p class="muted small">${esc(g.category)} · ${result}</p>
              <p class="small">Completed ${new Date(g.completed_at).toLocaleDateString()} · ${took}</p>
            </div>
            <div class="trophy-actions">${goalButtons(g)}</div>
          </article>`;
        })
        .join('')
    : '<p class="muted">Completed goals land here. Go earn your first trophy!</p>';
  const badges = v.badges
    .map(
      (b) => `<div class="badge ${b.unlocked_at ? 'on' : ''}" title="${b.unlocked_at ? `Unlocked ${new Date(b.unlocked_at).toLocaleDateString()}` : 'Locked'}">
        <div class="badge-icon">${b.unlocked_at ? b.icon : '🔒'}</div><b>${esc(b.name)}</b><small>${esc(b.description)}</small>
      </div>`,
    )
    .join('');
  return `<div class="stats">${stats}</div>
    <h2 class="section-title">Trophies</h2><div class="trophy-list">${trophies}</div>
    <h2 class="section-title">Badges <span class="muted">${unlocked}/${v.badges.length}</span></h2><div class="badges">${badges}</div>`;
}

// ---------- Game feedback ----------

function playEvents(events) {
  for (const e of events) {
    switch (e.type) {
      case 'xp':
        floatText(`+${e.amount} XP${e.lucky ? ' 🍀' : ''}`);
        if (e.lucky) toast('🍀 <b>Lucky!</b> Double XP on this update', 'lucky');
        break;
      case 'milestone':
        toast(`🎯 <b>${e.percent}%</b> on ${esc(e.goal)}<span class="xp">+${e.xp} XP</span>`);
        confetti(70, lastClick);
        break;
      case 'personal_best':
        toast(`🏃 <b>New personal best</b> on ${esc(e.goal)}<span class="xp">+${e.xp} XP</span>`, 'gold');
        confetti(90, lastClick);
        break;
      case 'quest_completed':
        toast(`🗺️ <b>Quest complete:</b> ${esc(e.quest)}<span class="xp">+${e.xp} XP</span>`, 'gold');
        break;
      case 'badge_unlocked':
        toast(`${e.icon} <b>Badge unlocked:</b> ${esc(e.name)}`, 'gold');
        break;
      case 'goal_completed':
        celebrate('🏆', 'Goal complete!', `${esc(e.goal)} · +${e.xp} XP`);
        break;
      case 'level_up':
        celebrate('⭐', `Level ${e.level}!`, `You are now a <b>${esc(e.title)}</b>`);
        break;
      case 'xp_revoked':
        toast(`↶ ${esc(e.reason)}${e.amount ? `<span class="xp">−${fmt(e.amount)} XP</span>` : ''}`);
        break;
    }
  }
}

function toast(html, kind = '') {
  const el = document.createElement('div');
  el.className = `toast ${kind}`;
  el.innerHTML = html;
  $('#toasts').append(el);
  setTimeout(() => {
    el.classList.add('out');
    setTimeout(() => el.remove(), 400);
  }, 4000);
}

function floatText(text) {
  const el = document.createElement('div');
  el.className = 'float-xp';
  el.textContent = text;
  el.style.left = `${lastClick.x}px`;
  el.style.top = `${lastClick.y}px`;
  document.body.append(el);
  setTimeout(() => el.remove(), 1300);
}

const celebrations = [];
let celebrationTimer = null;

function celebrate(icon, title, sub) {
  celebrations.push({ icon, title, sub });
  if ($('#celebration').classList.contains('hidden')) nextCelebration();
}

function nextCelebration() {
  clearTimeout(celebrationTimer);
  const el = $('#celebration');
  const item = celebrations.shift();
  if (!item) {
    el.classList.add('hidden');
    return;
  }
  el.innerHTML = `<div class="celebration-card">
    <div class="celebration-icon">${item.icon}</div>
    <h2>${item.title}</h2><p>${item.sub}</p><small class="muted">click to continue</small></div>`;
  el.classList.remove('hidden');
  confetti(240, { x: innerWidth / 2, y: innerHeight * 0.4 });
  celebrationTimer = setTimeout(nextCelebration, 4000);
}

// ---------- Confetti ----------

const canvas = $('#confetti');
const ctx = canvas.getContext('2d');
const COLORS = ['#8b5cf6', '#22d3ee', '#fbbf24', '#34d399', '#f472b6', '#f87171'];
let pieces = [];

function confetti(count, origin) {
  canvas.width = innerWidth;
  canvas.height = innerHeight;
  const start = pieces.length === 0;
  for (let i = 0; i < count; i++) {
    pieces.push({
      x: origin.x + (Math.random() - 0.5) * 60,
      y: origin.y,
      vx: (Math.random() - 0.5) * 16,
      vy: -Math.random() * 15 - 4,
      rot: Math.random() * Math.PI,
      spin: (Math.random() - 0.5) * 0.3,
      w: 6 + Math.random() * 6,
      h: 8 + Math.random() * 8,
      color: COLORS[i % COLORS.length],
      age: 0,
    });
  }
  if (start) requestAnimationFrame(tick);
}

function tick() {
  ctx.clearRect(0, 0, canvas.width, canvas.height);
  pieces = pieces.filter((p) => p.y < canvas.height + 40 && p.age < 400);
  for (const p of pieces) {
    p.vy += 0.35;
    p.vx *= 0.99;
    p.x += p.vx;
    p.y += p.vy;
    p.rot += p.spin;
    p.age++;
    ctx.save();
    ctx.translate(p.x, p.y);
    ctx.rotate(p.rot);
    ctx.fillStyle = p.color;
    ctx.fillRect(-p.w / 2, -p.h / 2, p.w, p.h * Math.abs(Math.cos(p.age / 8)));
    ctx.restore();
  }
  if (pieces.length) requestAnimationFrame(tick);
}

// ---------- Interaction ----------

function shake(el) {
  el.classList.remove('shake');
  void el.offsetWidth;
  el.classList.add('shake');
  el.focus();
}

/** Undo and delete can't be reversed, so they need a second click within 3 seconds. */
function confirmClick(el, verb, onConfirm) {
  if (el.dataset.armed) return onConfirm();
  const original = el.textContent;
  const xp = Number(el.dataset.xp);
  el.dataset.armed = '1';
  el.classList.add('armed');
  el.textContent = xp ? `${verb}? −${fmt(xp)} XP` : `${verb}?`;
  setTimeout(() => {
    if (!el.isConnected) return;
    delete el.dataset.armed;
    el.classList.remove('armed');
    el.textContent = original;
  }, 3000);
}

function logRun(el, id) {
  const box = el.parentElement;
  const kmInput = box.querySelector('.km');
  const timeInput = box.querySelector('.time');
  const distance = parseFloat(kmInput.value);
  const minutes = parseTime(timeInput.value);
  if (!(distance > 0)) return shake(kmInput);
  if (!(minutes > 0)) return shake(timeInput);
  call('log_run', { id, distance, minutes });
}

function setKind(kind) {
  state.kind = kind;
  document.querySelectorAll('#kind-toggle button').forEach((b) => b.classList.toggle('on', b.dataset.kind === kind));
  document.querySelectorAll('.kind-fields').forEach((f) => f.classList.toggle('hidden', f.dataset.for !== kind));
}

function openModal() {
  const form = $('#goal-form');
  form.reset();
  setKind('counter');
  $('#form-error').textContent = '';
  $('#category-list').innerHTML = state.view.categories.map((c) => `<option value="${esc(c)}">`).join('');
  $('#modal').classList.remove('hidden');
  form.elements.namedItem('name').focus();
}

const closeModal = () => $('#modal').classList.add('hidden');

document.addEventListener('click', (e) => {
  // Keyboard-triggered clicks have no coordinates; use the element's position instead.
  const rect = e.target.getBoundingClientRect();
  lastClick = e.detail ? { x: e.clientX, y: e.clientY } : { x: rect.left + rect.width / 2, y: rect.top };

  if (e.target.id === 'modal') return closeModal();
  if (e.target.closest('#celebration')) return nextCelebration();

  const tab = e.target.closest('[data-tab]');
  if (tab) {
    state.tab = tab.dataset.tab;
    return render();
  }

  const kindBtn = e.target.closest('[data-kind]');
  if (kindBtn) return setKind(kindBtn.dataset.kind);

  const el = e.target.closest('[data-action]');
  if (!el) return;
  const id = Number(el.dataset.id);
  switch (el.dataset.action) {
    case 'filter':
      state.filter = el.dataset.cat;
      return render();
    case 'new-goal':
      return openModal();
    case 'close-modal':
      return closeModal();
    case 'add':
      return call('add_progress', { id, delta: Number(el.dataset.delta) });
    case 'add-custom':
    case 'set-total': {
      const input = el.parentElement.querySelector('.amount');
      const value = parseFloat(input.value);
      if (!Number.isFinite(value)) return shake(input);
      return el.dataset.action === 'add-custom'
        ? call('add_progress', { id, delta: value })
        : call('set_progress', { id, total: value });
    }
    case 'log-run':
      return logRun(el, id);
    case 'undo':
      return confirmClick(el, 'Undo', () => call('undo_entry', { id }));
    case 'delete':
      return confirmClick(el, 'Delete', () => call('delete_goal', { id }));
    case 'archive':
      return call('archive_goal', { id });
    case 'restore':
      return call('restore_goal', { id });
    case 'toggle-archive':
      state.archiveOpen = !state.archiveOpen;
      return render();
  }
});

document.addEventListener('keydown', (e) => {
  if (e.key === 'Escape') return closeModal();
  if (e.key !== 'Enter') return;
  const input = e.target.closest?.('.actions input');
  if (!input) return;
  e.preventDefault();
  const action = input.classList.contains('amount') ? 'add-custom' : 'log-run';
  input.parentElement.querySelector(`[data-action="${action}"]`).click();
});

$('#goal-form').addEventListener('submit', async (e) => {
  e.preventDefault();
  const field = (n) => e.target.elements.namedItem(n).value.trim();
  const num = (n) => (field(n) === '' ? null : Number(field(n)));
  const input = { name: field('name'), category: field('category'), kind: state.kind, deadline: field('deadline') || null };
  if (state.kind === 'counter') {
    Object.assign(input, { target: num('target'), unit: field('unit'), start_value: num('start_value') });
  } else {
    const minutes = parseTime(field('time'));
    Object.assign(input, { distance_km: num('distance'), time_min: minutes > 0 ? minutes : null });
  }
  try {
    const res = await invoke('create_goal', { input });
    state.view = res.view;
    state.tab = 'goals';
    closeModal();
    render();
    toast(`🎯 New goal: <b>${esc(input.name)}</b>`);
  } catch (err) {
    $('#form-error').textContent = String(err);
  }
});

// Refresh now and then so the ghost, pace rival and week rollover stay current,
// but never while the user is typing.
setInterval(() => {
  const typing = document.activeElement?.tagName === 'INPUT';
  if (!typing && $('#modal').classList.contains('hidden')) call('get_state');
}, 5 * 60 * 1000);

call('get_state');
