// Real-user monitoring beacon. Inlined into every page's <head> by
// app.rs's `shell` — plain JS rather than part of the wasm bundle on
// purpose, so it still reports on pages whose hydration crashed (which is
// exactly when the wasm can't be trusted to run). Sends one anonymous
// JSON report per page load to /api/perf when the page is hidden/closed
// (or after 20s as a fallback); see perf.rs for what the server accepts.
(function () {
  if (!("PerformanceObserver" in window) || navigator.doNotTrack === "1") return;

  var m = { cls: 0 };
  var sent = false;

  function noteError(msg) {
    if (!m.js_error) m.js_error = String(msg).slice(0, 400);
  }
  addEventListener("error", function (e) {
    noteError(e.message || e.error);
  });
  addEventListener("unhandledrejection", function (e) {
    noteError(e.reason);
  });
  // console_error_panic_hook reports a wasm panic via console.error, not
  // via a thrown error the page can catch.
  var originalConsoleError = console.error;
  console.error = function () {
    try {
      if (typeof arguments[0] === "string" && arguments[0].indexOf("panicked") === 0) {
        noteError(arguments[0]);
      }
    } catch (_) {}
    return originalConsoleError.apply(console, arguments);
  };

  function observe(type, callback, options) {
    try {
      var observer = new PerformanceObserver(function (list) {
        list.getEntries().forEach(callback);
      });
      var init = options || {};
      init.type = type;
      init.buffered = true;
      observer.observe(init);
    } catch (_) {}
  }

  observe("paint", function (e) {
    if (e.name === "first-contentful-paint") m.fcp_ms = e.startTime;
  });
  observe("largest-contentful-paint", function (e) {
    m.lcp_ms = e.startTime;
  });
  observe("layout-shift", function (e) {
    if (!e.hadRecentInput) m.cls += e.value;
  });
  observe(
    "event",
    function (e) {
      if (e.interactionId && (!m.inp_ms || e.duration > m.inp_ms)) m.inp_ms = e.duration;
    },
    { durationThreshold: 40 },
  );

  // hydrate() in lib.rs sets this attribute once hydration finished.
  var hydrationWatcher = new MutationObserver(function () {
    if (document.body && document.body.getAttribute("data-hydrated") === "true") {
      m.hydrated_ms = performance.now();
      hydrationWatcher.disconnect();
    }
  });
  hydrationWatcher.observe(document.documentElement, {
    attributes: true,
    subtree: true,
    attributeFilter: ["data-hydrated"],
  });

  function round(n) {
    return typeof n === "number" && isFinite(n) ? Math.round(n) : undefined;
  }

  function report() {
    var nav = performance.getEntriesByType("navigation")[0] || {};
    var wasm = performance
      .getEntriesByType("resource")
      .filter(function (r) {
        return r.name.indexOf(".wasm") !== -1;
      })[0];
    var timing = {};
    (nav.serverTiming || []).forEach(function (t) {
      timing[t.name] = t.duration;
    });
    return {
      path: location.pathname,
      nav_type: nav.type,
      ttfb_ms: round(nav.responseStart),
      fcp_ms: round(m.fcp_ms),
      lcp_ms: round(m.lcp_ms),
      dom_ready_ms: round(nav.domContentLoadedEventEnd),
      load_ms: round(nav.loadEventEnd) || undefined,
      hydrated_ms: round(m.hydrated_ms),
      cls: Math.round(m.cls * 1000) / 1000,
      inp_ms: round(m.inp_ms),
      server_ms: round(timing.app),
      server_uptime_s: round(timing.up),
      wasm_transfer_bytes: wasm ? round(wasm.transferSize) : undefined,
      wasm_decoded_bytes: wasm ? round(wasm.decodedBodySize) : undefined,
      wasm_fetch_ms: wasm ? round(wasm.duration) : undefined,
      connection: navigator.connection && navigator.connection.effectiveType,
      viewport_w: innerWidth,
      js_error: m.js_error,
    };
  }

  function send() {
    if (sent) return;
    sent = true;
    var body = JSON.stringify(report());
    try {
      navigator.sendBeacon("/api/perf", new Blob([body], { type: "application/json" }));
    } catch (_) {}
  }

  addEventListener("visibilitychange", function () {
    if (document.visibilityState === "hidden") send();
  });
  addEventListener("pagehide", send);
  addEventListener("load", function () {
    setTimeout(send, 20000);
  });
  // For tests and manual checks.
  window.__rumFlush = send;
})();
