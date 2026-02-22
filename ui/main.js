const { listen } = window.__TAURI__.event;
const { invoke } = window.__TAURI__.core;

const urlInput = document.getElementById('url-input');
const goBtn = document.getElementById('go-btn');
const clearBtn = document.getElementById('clear-btn');
const feed = document.getElementById('feed');
const stats = document.getElementById('stats');
const alerts = document.getElementById('alerts');

const appSelector = document.getElementById('app-selector');

let requests = [];
let pendingDomains = new Set(); // domains we've already prompted for
let domainQueue = []; // queued unknown domains waiting for modal
let modalActive = false; // is a naming modal currently showing

// --- Feed filter state ---
let feedFilterAuth = true; // default: show auth-only requests
let totalCount = 0;
let filteredCount = 0;

// --- Active label tracking ---
let activeLabel = null;       // the current label text
let labelCloseTimer = null;   // countdown interval
let labelCloseTimeout = null; // main timer
const LABEL_INACTIVITY = 8; // seconds of NO new captures before showing close prompt

// --- App Selector ---
async function loadAppSelector() {
  try {
    const apps = await invoke('get_app_details');
    appSelector.innerHTML = '';
    if (!apps || apps.length === 0) {
      appSelector.style.display = 'none';
      return;
    }
    appSelector.style.display = 'flex';
    apps.forEach(app => {
      const primaryDomain = app.domains && app.domains.length > 0 ? app.domains[0] : '';
      const btn = document.createElement('button');
      btn.className = 'app-btn';
      btn.innerHTML = `<span class="app-btn-name">${esc(app.name)}</span>` +
        (primaryDomain ? `<span class="app-btn-domain">${esc(primaryDomain)}</span>` : '');
      btn.title = primaryDomain;
      btn.addEventListener('click', () => {
        if (primaryDomain) {
          urlInput.value = primaryDomain;
          go(); // goes through annotation-first flow
        }
      });
      appSelector.appendChild(btn);
    });
  } catch (e) {
    appSelector.style.display = 'none';
  }
}

// Load apps on startup
loadAppSelector();

// Refresh app selector when a new app is registered
listen('config-updated', () => loadAppSelector());

// --- Annotation ---
function submitAnnotation() {
  const input = document.getElementById('annotate-input');
  const label = input.value.trim();
  input.classList.remove('pending');

  if (pendingNavigateUrl) {
    const url = pendingNavigateUrl;
    pendingNavigateUrl = null;
    if (label) {
      // Close any active label first, then start the new one
      closeActiveLabel();
      invoke('annotate_action', { label });
      startLabelLifecycle(label);
      input.value = '';
      input.placeholder = '\u2713 ' + label;
      setTimeout(() => { input.placeholder = 'Label what you\'re about to do...'; }, 2000);
    } else {
      input.placeholder = 'Label what you\'re about to do...';
    }
    invoke('navigate', { url });
    return;
  }

  // Standalone annotation (no pending navigate)
  if (!label) return;
  closeActiveLabel();
  invoke('annotate_action', { label });
  startLabelLifecycle(label);
  input.value = '';
  input.placeholder = '\u2713 ' + label;
  setTimeout(() => { input.placeholder = 'Label what you\'re about to do...'; }, 2000);
}

document.getElementById('annotate-btn').addEventListener('click', submitAnnotation);

document.getElementById('annotate-input').addEventListener('keydown', (e) => {
  if (e.key === 'Enter') {
    submitAnnotation();
  } else if (e.key === 'Escape' && pendingNavigateUrl) {
    const input = document.getElementById('annotate-input');
    input.classList.remove('pending');
    input.value = '';
    input.placeholder = 'Label what you\'re about to do...';
    const url = pendingNavigateUrl;
    pendingNavigateUrl = null;
    invoke('navigate', { url });
  }
});

// --- Label lifecycle: active label → timer → close prompt → done ---

function startLabelLifecycle(label) {
  activeLabel = label;
  // Show close prompt after LABEL_INACTIVITY seconds of no new captures.
  // Every new capture resets the timer — so busy workflows stay open.
  resetLabelInactivityTimer();
}

function resetLabelInactivityTimer() {
  if (!activeLabel) return;
  clearTimeout(labelCloseTimeout);
  labelCloseTimeout = setTimeout(() => showLabelClosePrompt(), LABEL_INACTIVITY * 1000);
}

function showLabelClosePrompt() {
  if (!activeLabel) return;
  const bar = document.getElementById('label-close-bar');
  const original = document.getElementById('label-close-original');
  const input = document.getElementById('label-close-input');
  const timerEl = document.getElementById('label-close-timer');

  original.textContent = activeLabel;
  original.title = activeLabel;
  input.value = '';
  input.placeholder = 'what actually happened? (or leave blank to keep)';
  bar.classList.remove('hidden');

  // 30 second countdown to auto-close
  let remaining = 30;
  timerEl.textContent = remaining + 's';
  clearInterval(labelCloseTimer);
  labelCloseTimer = setInterval(() => {
    remaining--;
    timerEl.textContent = remaining + 's';
    if (remaining <= 0) {
      closeLabelPrompt(null); // auto-close with original label
    }
  }, 1000);

  input.focus();
}

function closeLabelPrompt(revised) {
  clearInterval(labelCloseTimer);
  clearTimeout(labelCloseTimeout);

  const bar = document.getElementById('label-close-bar');
  bar.classList.add('hidden');

  if (activeLabel) {
    const finalLabel = revised || activeLabel;
    // Save a close annotation so the digest knows when the workflow ended
    // and what actually happened
    invoke('annotate_action', { label: '[done] ' + finalLabel });
  }
  activeLabel = null;
}

// Silently close the active label (when starting a new one)
function closeActiveLabel() {
  if (!activeLabel) return;
  clearInterval(labelCloseTimer);
  clearTimeout(labelCloseTimeout);
  // Save close with original label — the new label supersedes it
  invoke('annotate_action', { label: '[done] ' + activeLabel });
  activeLabel = null;
  document.getElementById('label-close-bar').classList.add('hidden');
}

document.getElementById('label-close-btn').addEventListener('click', () => {
  const input = document.getElementById('label-close-input');
  const revised = input.value.trim();
  closeLabelPrompt(revised || null);
});

document.getElementById('label-close-input').addEventListener('keydown', (e) => {
  if (e.key === 'Enter') {
    const input = document.getElementById('label-close-input');
    const revised = input.value.trim();
    closeLabelPrompt(revised || null);
  } else if (e.key === 'Escape') {
    closeLabelPrompt(null); // keep original
  }
});

// --- Feed filter toggle ---
document.getElementById('feed-toggle').addEventListener('click', () => {
  feedFilterAuth = !feedFilterAuth;
  const btn = document.getElementById('feed-toggle');
  btn.textContent = feedFilterAuth ? 'Auth Only' : 'All';
  btn.classList.toggle('active', feedFilterAuth);
  updateFeedStats();
});

function hasAuthHeaders(data) {
  const headers = data.requestHeaders || {};
  for (const key of Object.keys(headers)) {
    const lower = key.toLowerCase();
    if (lower === 'authorization' || lower === 'x-csrf-token' || lower === 'x-xsrf-token') return true;
  }
  const cookies = headers['cookie'] || headers['Cookie'] || '';
  if (cookies.length > 20) return true;
  return false;
}

function updateFeedStats() {
  const statsEl = document.getElementById('feed-stats');
  if (feedFilterAuth) {
    const shown = totalCount - filteredCount;
    statsEl.textContent = `Showing ${shown} of ${totalCount} requests (auth only)`;
  } else {
    statsEl.textContent = `Showing ${totalCount} of ${totalCount} requests (all)`;
  }
}

// --- Navigate ---
// Navigation is blocked until a label is set (annotation-first workflow).
// The annotation bar gets focus. User types what they're about to do, then
// pressing Enter in the annotation bar both saves the label and navigates.
let pendingNavigateUrl = null;

function go() {
  let url = urlInput.value.trim();
  if (!url) return;

  const annotateInput = document.getElementById('annotate-input');
  const existingLabel = annotateInput.value.trim();

  if (existingLabel) {
    // Label already set — annotate and navigate immediately
    invoke('annotate_action', { label: existingLabel });
    annotateInput.value = '';
    annotateInput.placeholder = '\u2713 ' + existingLabel;
    setTimeout(() => { annotateInput.placeholder = 'Label what you\'re about to do...'; }, 2000);
    pendingNavigateUrl = null;
    invoke('navigate', { url });
  } else {
    // No label — block and focus annotation bar
    pendingNavigateUrl = url;
    annotateInput.placeholder = 'What are you about to do? (Enter to go, Esc to skip)';
    annotateInput.focus();
    annotateInput.classList.add('pending');
  }
}

goBtn.addEventListener('click', go);
urlInput.addEventListener('keydown', e => { if (e.key === 'Enter') go(); });
clearBtn.addEventListener('click', () => {
  requests = [];
  feed.innerHTML = '';
  updateStats();
});

// --- End Session ---
document.getElementById('end-session-btn').addEventListener('click', async () => {
  const btn = document.getElementById('end-session-btn');
  btn.textContent = '...';
  btn.classList.add('processing');
  closeActiveLabel();
  try {
    await invoke('end_session');
    showNotice('Session finalized — digest and endpoints updated');
  } catch (e) {
    showNotice('Error: ' + e);
  }
  btn.textContent = 'Done';
  btn.classList.remove('processing');
});

// --- Check UA on startup ---
(async function checkConfig() {
  const config = await invoke('get_config');
  if (!config.user_agent) {
    showAlert(
      'No Chrome user-agent configured. Paste yours from Chrome DevTools (console → <code>navigator.userAgent</code>):',
      'ua-setup',
      async (value) => {
        try {
          await invoke('set_user_agent', { ua: value });
          showNotice('User-agent saved. Restart harharhar for it to take effect.');
        } catch (err) {
          showNotice('Error: ' + err);
        }
      }
    );
  }
})();

// --- Listen for captured requests ---
listen('request-captured', event => {
  const req = event.payload;
  // Every capture = activity → reset the label inactivity timer
  resetLabelInactivityTimer();

  // Don't show noise entries in the feed
  if (req.type === 'cookies' || req.type === 'navigation' || req.type === 'xhr-start' || req.type === 'annotation' || (req.type || '').startsWith('perf-')) return;
  totalCount++;
  if (feedFilterAuth && !hasAuthHeaders(req)) {
    filteredCount++;
    updateFeedStats();
    return;
  }
  requests.unshift(req);
  renderReq(req);
  updateStats();
  updateFeedStats();
});

// --- Pre-navigate naming: blocks browser until app is named ---
listen('name-app-before-navigate', event => {
  const domain = event.payload;
  if (pendingDomains.has(domain)) return;
  pendingDomains.add(domain);

  let suggested = domain
    .replace(/^(www|app|api|mail)\./, '')
    .replace(/\.(com|org|net|io|dev|co)$/, '')
    .replace(/\./g, '-');

  showNamingModal(domain, suggested, async (name) => {
    await invoke('register_app', { name, domain });
    showNotice(`App "${name}" created for ${domain}`);
    loadAppSelector();
    await invoke('resume_navigate');
  });
});

// --- Fallback: captures from unmapped domains (redirects, subdomains) ---
let _domainBatchTimer = null;
listen('unknown-domain', event => {
  const domain = event.payload;
  if (pendingDomains.has(domain)) return;
  pendingDomains.add(domain);
  domainQueue.push(domain);
  // Batch domains arriving within 1s into one modal
  clearTimeout(_domainBatchTimer);
  _domainBatchTimer = setTimeout(processNextDomain, 1000);
});

async function processNextDomain() {
  if (modalActive || domainQueue.length === 0) return;
  modalActive = true;

  // Batch all queued domains into one modal
  const domains = domainQueue.splice(0, domainQueue.length);
  const apps = await invoke('get_apps');

  if (apps.length > 0) {
    showAddDomainModal(domains, apps, async (appName) => {
      for (const d of domains) {
        await invoke('add_domain', { name: appName, domain: d });
      }
      showNotice(`Added ${domains.length} domain(s) to "${appName}"`);
      loadAppSelector();
      modalActive = false;
      processNextDomain();
    });
  } else {
    const suggested = domains[0]
      .replace(/^(www|app|api|mail)\./, '')
      .replace(/\.(com|org|net|io|dev|co)$/, '')
      .replace(/\./g, '-');

    showNamingModal(domains[0], suggested, async (name) => {
      await invoke('register_app', { name, domain: domains[0] });
      for (const d of domains.slice(1)) {
        await invoke('add_domain', { name, domain: d });
      }
      showNotice(`App "${name}" created with ${domains.length} domain(s)`);
      loadAppSelector();
      modalActive = false;
      processNextDomain();
    });
  }
}

// --- Modal naming dialog (blocks everything until answered) ---
function showNamingModal(domain, suggested, onSubmit) {
  // Remove any existing modal
  const existing = document.getElementById('naming-modal');
  if (existing) existing.remove();

  const overlay = document.createElement('div');
  overlay.id = 'naming-modal';
  overlay.className = 'modal-overlay';
  overlay.innerHTML = `
    <div class="modal">
      <div class="modal-title">Name this app</div>
      <div class="modal-domain">${esc(domain)}</div>
      <div class="modal-hint">Choose a short name for this app (e.g. "gmail", "jira", "slack")</div>
      <input class="modal-input" type="text" value="${esc(suggested)}" spellcheck="false" autocomplete="off">
      <div class="modal-actions">
        <button class="modal-save">Save &amp; Continue</button>
      </div>
    </div>
  `;

  const input = overlay.querySelector('.modal-input');
  const btn = overlay.querySelector('.modal-save');

  function submit() {
    const name = input.value.trim().toLowerCase().replace(/[^a-z0-9-]/g, '-');
    if (!name) { input.focus(); return; }
    overlay.remove();
    onSubmit(name);
  }

  btn.addEventListener('click', submit);
  input.addEventListener('keydown', e => { if (e.key === 'Enter') submit(); });

  document.body.appendChild(overlay);
  input.focus();
  input.select();
}

// --- Modal: add domain(s) to existing app OR create new ---
function showAddDomainModal(domains, apps, onSubmit) {
  const existing = document.getElementById('naming-modal');
  if (existing) existing.remove();

  const domainList = Array.isArray(domains) ? domains : [domains];

  const appBtns = apps.map(a =>
    `<button class="modal-app-btn" data-app="${esc(a)}">${esc(a)}</button>`
  ).join('');

  const domainDisplay = domainList.map(d => `<div class="modal-domain">${esc(d)}</div>`).join('');

  const overlay = document.createElement('div');
  overlay.id = 'naming-modal';
  overlay.className = 'modal-overlay';
  overlay.innerHTML = `
    <div class="modal">
      <div class="modal-title">New domain${domainList.length > 1 ? 's' : ''} detected</div>
      ${domainDisplay}
      <div class="modal-hint">Add to an existing app:</div>
      <div class="modal-app-list">${appBtns}</div>
      <div class="modal-divider">or create new</div>
      <input class="modal-input" type="text" placeholder="new app name..." spellcheck="false" autocomplete="off">
      <div class="modal-actions">
        <button class="modal-save">Create</button>
        <button class="modal-dismiss-btn">Skip</button>
      </div>
    </div>
  `;

  // Add to existing app
  overlay.querySelectorAll('.modal-app-btn').forEach(btn => {
    btn.addEventListener('click', () => {
      overlay.remove();
      onSubmit(btn.dataset.app);
    });
  });

  // Create new app
  const input = overlay.querySelector('.modal-input');
  const createBtn = overlay.querySelector('.modal-save');
  function createNew() {
    const name = input.value.trim().toLowerCase().replace(/[^a-z0-9-]/g, '-');
    if (!name) { input.focus(); return; }
    overlay.remove();
    invoke('register_app', { name, domain: domainList[0] }).then(async () => {
      for (const d of domainList.slice(1)) {
        await invoke('add_domain', { name, domain: d });
      }
      showNotice(`App "${name}" created with ${domainList.length} domain(s)`);
      modalActive = false;
      processNextDomain();
    });
  }
  createBtn.addEventListener('click', createNew);
  input.addEventListener('keydown', e => { if (e.key === 'Enter') createNew(); });

  // Skip
  overlay.querySelector('.modal-dismiss-btn').addEventListener('click', () => {
    overlay.remove();
    modalActive = false;
    processNextDomain();
  });

  document.body.appendChild(overlay);
}

// --- Alert/prompt system ---
function showAlert(message, id, onSubmit, defaultValue) {
  // Don't show duplicate alerts
  if (document.getElementById(`alert-${id}`)) return;

  const el = document.createElement('div');
  el.className = 'alert';
  el.id = `alert-${id}`;
  el.innerHTML = `
    <div class="alert-msg">${message}</div>
    <div class="alert-row">
      <input class="alert-input" type="text" value="${esc(defaultValue || '')}" spellcheck="false">
      <button class="alert-btn">Save</button>
      <button class="alert-dismiss">✕</button>
    </div>
  `;

  const input = el.querySelector('.alert-input');
  const btn = el.querySelector('.alert-btn');
  const dismiss = el.querySelector('.alert-dismiss');

  function submit() {
    const val = input.value.trim();
    if (val) {
      onSubmit(val);
      el.remove();
    }
  }

  btn.addEventListener('click', submit);
  input.addEventListener('keydown', e => { if (e.key === 'Enter') submit(); });
  dismiss.addEventListener('click', () => el.remove());

  alerts.prepend(el);
  input.focus();
  input.select();
}

function showNotice(text) {
  const el = document.createElement('div');
  el.className = 'notice';
  el.textContent = text;
  alerts.prepend(el);
  setTimeout(() => el.remove(), 4000);
}

// --- Stats ---
function updateStats() {
  const methods = {};
  requests.forEach(r => { methods[r.method] = (methods[r.method] || 0) + 1; });
  const parts = Object.entries(methods).map(([m, c]) => `${m}:${c}`);
  stats.textContent = `${requests.length} requests captured` + (parts.length ? ` (${parts.join(', ')})` : '');
}

// --- Request rendering ---
function renderReq(req) {
  const row = document.createElement('div');
  row.className = 'req';

  let displayPath = req.url;
  try {
    const u = new URL(req.url);
    displayPath = u.pathname + u.search;
  } catch (e) {}

  const sc = req.status < 400 ? 'ok' : 'err';

  row.innerHTML = `
    <span class="method ${req.method}">${req.method}</span>
    <span class="path" title="${esc(req.url)}">${esc(displayPath)}</span>
    <span class="status ${sc}">${req.status}</span>
    <span class="dur">${req.duration}ms</span>
  `;

  row.addEventListener('click', () => toggleDetail(row, req));
  feed.prepend(row);
}

function toggleDetail(row, req) {
  const next = row.nextElementSibling;
  if (next && next.classList.contains('detail')) {
    next.remove();
    return;
  }

  document.querySelectorAll('.detail').forEach(d => d.remove());

  const d = document.createElement('div');
  d.className = 'detail';

  const curl = buildCurl(req);
  const reqBody = tryPretty(req.requestBody);
  const resBody = tryPretty(req.responseBody);

  d.innerHTML = `
    <div class="label">URL</div>
    <div>${esc(req.url)}</div>
    <div class="label">cURL <button class="copy-btn" data-copy="${esc(curl)}">copy</button></div>
    <div>${esc(curl)}</div>
    ${reqBody ? `<div class="label">Request Body</div><div>${esc(reqBody)}</div>` : ''}
    <div class="label">Request Headers</div>
    <div>${fmtHeaders(req.requestHeaders)}</div>
    <div class="label">Response Headers</div>
    <div>${fmtHeaders(req.responseHeaders)}</div>
    ${resBody ? `<div class="label">Response Body</div><div>${esc(resBody)}</div>` : ''}
  `;

  d.querySelectorAll('.copy-btn').forEach(btn => {
    btn.addEventListener('click', e => {
      e.stopPropagation();
      navigator.clipboard.writeText(btn.dataset.copy);
      btn.textContent = 'copied';
      setTimeout(() => btn.textContent = 'copy', 1000);
    });
  });

  row.after(d);
}

function buildCurl(req) {
  let c = `curl_chrome '${req.url}'`;
  if (req.method !== 'GET') c += ` -X ${req.method}`;
  if (req.requestHeaders) {
    for (const [k, v] of Object.entries(req.requestHeaders)) {
      c += ` \\\n  -H '${k}: ${v}'`;
    }
  }
  if (req.requestBody) c += ` \\\n  -d '${req.requestBody}'`;
  return c;
}

function tryPretty(str) {
  if (!str) return null;
  try { return JSON.stringify(JSON.parse(str), null, 2); } catch (e) { return str; }
}

function fmtHeaders(h) {
  if (!h || typeof h !== 'object') return '-';
  return Object.entries(h).map(([k, v]) => `${esc(k)}: ${esc(v)}`).join('\n');
}

function esc(s) {
  if (!s) return '';
  return String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
}
