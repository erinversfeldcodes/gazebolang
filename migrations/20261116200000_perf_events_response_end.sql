-- When the whole HTML response had arrived (PerformanceNavigationTiming's
-- responseEnd). `server_ms` only covers the time to the first byte of the
-- response, and Leptos streams the page shell immediately and fills in
-- data-dependent sections afterwards — so first byte is near zero no
-- matter how slow the queries are. This is the metric that includes them.
ALTER TABLE perf_events ADD COLUMN response_end_ms INTEGER;
