(function () {
  'use strict';

  // --- Capture buffer: queues entries until Tauri IPC is ready ---
  const _buffer = [];
  const _capturedUrls = new Set(); // dedup perf entries

  function send(entry) {
    try {
      if (window.__TAURI_INTERNALS__) {
        // Flush buffered entries first
        while (_buffer.length > 0) {
          window.__TAURI_INTERNALS__.invoke('save_capture_data', { data: _buffer.shift() });
        }
        window.__TAURI_INTERNALS__.invoke('save_capture_data', { data: entry });
      } else {
        _buffer.push(entry);
      }
    } catch (_) {
      _buffer.push(entry);
    }
  }

  // Retry flushing buffer until IPC is ready (check every 50ms for up to 30s)
  const _flushTimer = setInterval(function () {
    if (window.__TAURI_INTERNALS__ && _buffer.length > 0) {
      while (_buffer.length > 0) {
        window.__TAURI_INTERNALS__.invoke('save_capture_data', { data: _buffer.shift() });
      }
    }
  }, 50);
  setTimeout(function () { clearInterval(_flushTimer); }, 30000);

  // --- Capture the page navigation itself ---
  send({
    type: 'navigation',
    method: 'GET',
    url: location.href,
    requestHeaders: { cookie: document.cookie || '' },
    requestBody: null,
    status: 0,
    statusText: '',
    responseHeaders: {},
    responseBody: null,
    duration: 0,
    timestamp: new Date().toISOString()
  });

  // --- fetch wrapper ---
  const _fetch = window.fetch.bind(window);

  window.fetch = async function (...args) {
    const req = new Request(...args);
    const reqForFetch = req.clone();

    const method = req.method;
    const url = req.url;
    const requestHeaders = {};
    req.headers.forEach(function (v, k) { requestHeaders[k] = v; });
    let requestBody = null;
    try { requestBody = await req.text(); } catch (_) {}

    _capturedUrls.add(url);

    const t0 = performance.now();
    try {
      const res = await _fetch(reqForFetch);
      const clone = res.clone();
      let responseBody = null;
      try { responseBody = (await clone.text()).substring(0, 500000); } catch (_) {}
      const responseHeaders = {};
      res.headers.forEach(function (v, k) { responseHeaders[k] = v; });

      send({ type: 'fetch', method: method, url: url, requestHeaders: requestHeaders,
        requestBody: requestBody, status: res.status, statusText: res.statusText,
        responseHeaders: responseHeaders, responseBody: responseBody,
        duration: Math.round(performance.now() - t0), timestamp: new Date().toISOString() });
      return res;
    } catch (err) {
      send({ type: 'fetch', method: method, url: url, requestHeaders: requestHeaders,
        requestBody: requestBody, status: 0, statusText: err.message,
        responseHeaders: {}, responseBody: null,
        duration: Math.round(performance.now() - t0), timestamp: new Date().toISOString() });
      throw err;
    }
  };

  // --- XHR wrapper ---
  const _open = XMLHttpRequest.prototype.open;
  const _send = XMLHttpRequest.prototype.send;
  const _setH = XMLHttpRequest.prototype.setRequestHeader;

  XMLHttpRequest.prototype.open = function (method, url) {
    this.__m = method;
    this.__u = new URL(url, location.href).href;
    this.__h = {};
    return _open.apply(this, arguments);
  };

  XMLHttpRequest.prototype.setRequestHeader = function (k, v) {
    if (this.__h) this.__h[k] = v;
    return _setH.call(this, k, v);
  };

  XMLHttpRequest.prototype.send = function (body) {
    var t0 = performance.now();
    var xhr = this;
    _capturedUrls.add(xhr.__u);

    // Send request-start immediately so we don't lose it on navigation
    send({ type: 'xhr-start', method: xhr.__m, url: xhr.__u,
      requestHeaders: xhr.__h || {},
      requestBody: body ? String(body).substring(0, 500000) : null,
      status: 0, statusText: 'pending', responseHeaders: {},
      responseBody: null, duration: 0, timestamp: new Date().toISOString() });

    xhr.addEventListener('loadend', function () {
      var rh = {};
      (xhr.getAllResponseHeaders() || '').split('\r\n').forEach(function (l) {
        var i = l.indexOf(': ');
        if (i > 0) rh[l.slice(0, i)] = l.slice(i + 2);
      });
      send({ type: 'xhr', method: xhr.__m, url: xhr.__u,
        requestHeaders: xhr.__h || {},
        requestBody: body ? String(body).substring(0, 500000) : null,
        status: xhr.status, statusText: xhr.statusText, responseHeaders: rh,
        responseBody: (xhr.responseText || '').substring(0, 500000),
        duration: Math.round(performance.now() - t0), timestamp: new Date().toISOString() });
    });
    return _send.call(this, body);
  };

  // --- Capture document.cookie once on load ---
  function captureCookies() {
    var cookies = document.cookie;
    if (!cookies) return;
    send({
      type: 'cookies',
      method: 'COOKIES',
      url: location.href,
      requestHeaders: { cookie: cookies },
      requestBody: null,
      status: 0,
      statusText: '',
      responseHeaders: {},
      responseBody: null,
      duration: 0,
      timestamp: new Date().toISOString()
    });
  }

  if (document.readyState === 'complete') {
    captureCookies();
  } else {
    window.addEventListener('load', captureCookies);
  }

  // --- Catch requests we missed via PerformanceObserver ---
  // This acts like Chrome's "preserve log" — catches requests that
  // fired before our wrappers were in place, or in-flight during nav
  function emitPerfEntry(entry) {
    // Only care about API-like requests, not images/scripts/css
    if (entry.initiatorType !== 'xmlhttprequest' && entry.initiatorType !== 'fetch') return;
    // Skip if our wrapper already captured it
    if (_capturedUrls.has(entry.name)) return;
    send({
      type: 'perf-' + entry.initiatorType,
      method: 'GET',
      url: entry.name,
      requestHeaders: {},
      requestBody: null,
      status: entry.responseStatus || 0,
      statusText: '',
      responseHeaders: {},
      responseBody: null,
      duration: Math.round(entry.duration),
      timestamp: new Date(performance.timeOrigin + entry.startTime).toISOString(),
      transferSize: entry.transferSize || 0
    });
  }

  // Observe future resource loads
  try {
    var observer = new PerformanceObserver(function (list) {
      var entries = list.getEntries();
      for (var i = 0; i < entries.length; i++) {
        emitPerfEntry(entries[i]);
      }
    });
    observer.observe({ type: 'resource', buffered: true });
  } catch (_) {}

  // --- WebSocket wrapper ---
  const _WS = window.WebSocket;
  const _wsSockets = [];
  window.__hh_ws = _wsSockets;

  window.WebSocket = function (url, protocols) {
    var ws = protocols ? new _WS(url, protocols) : new _WS(url);
    _wsSockets.push(ws);

    send({ type: 'ws-open', method: 'WS', url: url, requestHeaders: {}, requestBody: null,
      status: 0, statusText: '', responseHeaders: {}, responseBody: null,
      duration: 0, timestamp: new Date().toISOString() });

    ws.addEventListener('message', function (e) {
      var data = typeof e.data === 'string' ? e.data.substring(0, 100000) : '[binary]';
      send({ type: 'ws-msg-in', method: 'WS', url: url, requestHeaders: {}, requestBody: null,
        status: 0, statusText: 'incoming', responseHeaders: {}, responseBody: data,
        duration: 0, timestamp: new Date().toISOString() });
    });

    var _wsSend = ws.send.bind(ws);
    ws.send = function (data) {
      var body = typeof data === 'string' ? data.substring(0, 100000) : '[binary]';
      send({ type: 'ws-msg-out', method: 'WS', url: url, requestHeaders: {}, requestBody: body,
        status: 0, statusText: 'outgoing', responseHeaders: {}, responseBody: null,
        duration: 0, timestamp: new Date().toISOString() });
      return _wsSend(data);
    };

    return ws;
  };
  window.WebSocket.prototype = _WS.prototype;
  window.WebSocket.CONNECTING = _WS.CONNECTING;
  window.WebSocket.OPEN = _WS.OPEN;
  window.WebSocket.CLOSING = _WS.CLOSING;
  window.WebSocket.CLOSED = _WS.CLOSED;

  console.log('[harharhar] intercept active — buffered capture enabled');
})();
