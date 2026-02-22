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
          invoke('navigate', { url: primaryDomain });
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

// --- Navigate ---
function go() {
  let url = urlInput.value.trim();
  if (!url) return;
  invoke('navigate', { url });
}

goBtn.addEventListener('click', go);
urlInput.addEventListener('keydown', e => { if (e.key === 'Enter') go(); });
clearBtn.addEventListener('click', () => {
  requests = [];
  feed.innerHTML = '';
  updateStats();
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
  // Don't show noise entries in the feed
  if (req.type === 'cookies' || req.type === 'navigation' || req.type === 'xhr-start' || (req.type || '').startsWith('perf-')) return;
  requests.unshift(req);
  renderReq(req);
  updateStats();
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
