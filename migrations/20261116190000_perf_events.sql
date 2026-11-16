-- Real-user page-load measurements, one row per page load, written by the
-- beacon in crates/web/src/rum.js via POST /api/perf. Anonymous by design:
-- no user id, IP or user agent is stored — only what's needed to compare
-- deploys and routes. `path` is the route pattern (UUIDs collapsed to
-- :id), `release` is the Fly machine version that served the request, so
-- before/after a deploy is a GROUP BY. All durations are milliseconds
-- since navigation start unless noted; NULL means the browser didn't
-- report that metric (or the page never got that far).
CREATE TABLE perf_events (
    id BIGSERIAL PRIMARY KEY,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    release TEXT NOT NULL,
    path TEXT NOT NULL,
    nav_type TEXT,
    ttfb_ms INTEGER,
    fcp_ms INTEGER,
    lcp_ms INTEGER,
    dom_ready_ms INTEGER,
    load_ms INTEGER,
    -- When body[data-hydrated] was set. NULL together with js_error set is
    -- a page whose hydration crashed.
    hydrated_ms INTEGER,
    cls REAL,
    -- Slowest interaction (approximates INP).
    inp_ms INTEGER,
    -- From the response's Server-Timing header: time the server took to
    -- produce the response headers, and how long the process had been up
    -- (small = a cold start just served this).
    server_ms INTEGER,
    server_uptime_s INTEGER,
    wasm_transfer_bytes INTEGER,
    wasm_decoded_bytes INTEGER,
    wasm_fetch_ms INTEGER,
    connection TEXT,
    viewport_w INTEGER,
    -- First uncaught error / wasm panic message on the page, truncated.
    js_error TEXT
);

CREATE INDEX perf_events_created_at_idx ON perf_events (created_at);
CREATE INDEX perf_events_release_path_idx ON perf_events (release, path);
