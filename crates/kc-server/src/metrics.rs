//! In-process observability: a dependency-free metrics registry, the request-id
//! generator and the `GET /metrics` handler.
//!
//! Nothing here talks to an external collector. The registry is a handful of
//! atomics rendered as Prometheus text on demand, so a scrape costs one pass over
//! the counters and no allocation per request. That is deliberate: this is a
//! single-user, self-hosted process, and pulling in a metrics SDK (plus its
//! background exporter) to answer "what is p99" and "who is hammering
//! `/api/events`" would be more moving parts than the question deserves.
//!
//! # What is recorded
//!
//! * `karucode_http_requests_total{method,status_class}` - every request that
//!   reached the router, counted in [`observability`] (the outermost layer), so a
//!   `401` from the auth guard and a `403` from the `Host` guard are counted too.
//! * `karucode_sse_connections_active` / `_total` / `_rejected_total` - live
//!   server-sent-event connections, how many were accepted, and how many were
//!   refused because [`MAX_SSE_CONNECTIONS`] was reached.
//! * `karucode_runs_active` - runs in flight, read from the runtime at scrape time.
//! * `karucode_runs_started_total` / `karucode_runs_finished_total` - run
//!   lifecycle counters, incremented through [`record_run_started`] and
//!   [`record_run_finished`].
//! * `karucode_uptime_seconds`, `karucode_build_info`.
//!
//! # Request ids
//!
//! Every request gets a fresh v4-shaped UUID in [`observability`], recorded on the
//! span and echoed back as `X-Request-Id`. The id is always minted, never taken
//! from the request: honouring an inbound header would let a client choose what
//! the access log says.
//!
//! # Access log
//!
//! One `info` line per request with method, path, status and latency. The path is
//! `uri().path()`, never the query string: `GET /api/events?token=...` carries a
//! bearer token, and a log is the last place that should end up. Bodies are never
//! read, let alone logged.

use crate::state::AppState;
use axum::body::Body;
use axum::extract::State;
use axum::http::{header, HeaderName, HeaderValue, Method, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// Hard cap on concurrent server-sent-event connections. Each one holds a task
/// and a queue, so an unbounded number is an unbounded amount of memory that any
/// client can ask for.
pub const MAX_SSE_CONNECTIONS: usize = 64;

/// Hard cap on the bytes queued for one SSE connection. A single
/// `tool_call_finished` frame can carry 24 000 bytes and `command_output` up to
/// 32 000, so bounding the queue by *slots* alone (1024 of them) allowed ~32 MB
/// per stalled client.
pub const MAX_QUEUED_BYTES: usize = 4 * 1024 * 1024;

/// Response header carrying the per-request correlation id.
pub const REQUEST_ID_HEADER: &str = "x-request-id";

/// The methods the request counter keeps a series for. Anything else is folded
/// into `OTHER`, so a client cannot grow the cardinality by inventing methods.
const METHODS: [&str; 8] = [
    "GET", "POST", "PATCH", "PUT", "DELETE", "HEAD", "OPTIONS", "OTHER",
];

/// Status classes, in the order [`class_index`] returns them.
const CLASSES: [&str; 5] = ["1xx", "2xx", "3xx", "4xx", "5xx"];

/// The correlation id minted for one request, carried in the request extensions
/// so the tracing layer and any handler can read it back.
#[derive(Clone, Debug)]
pub struct RequestId(pub String);

/// The values a scrape needs that live outside the registry.
#[derive(Clone, Copy, Debug)]
pub struct MetricsSnapshot {
    pub active_runs: usize,
    pub uptime_secs: u64,
}

/// Counters and gauges for one process.
pub struct Metrics {
    /// `[method][status class]`, see [`METHODS`] and [`CLASSES`].
    requests: [[AtomicU64; 5]; 8],
    sse_active: AtomicU64,
    sse_total: AtomicU64,
    sse_rejected: AtomicU64,
}

impl Default for Metrics {
    fn default() -> Self {
        Self {
            requests: std::array::from_fn(|_| std::array::from_fn(|_| AtomicU64::new(0))),
            sse_active: AtomicU64::new(0),
            sse_total: AtomicU64::new(0),
            sse_rejected: AtomicU64::new(0),
        }
    }
}

/// The process-wide registry.
///
/// A `static` rather than an `AppState` field: `state.rs` is owned by another
/// change. Moving it onto `AppState` later is a mechanical swap - every caller
/// goes through [`registry`].
pub fn registry() -> &'static Metrics {
    static REGISTRY: OnceLock<Metrics> = OnceLock::new();
    REGISTRY.get_or_init(Metrics::default)
}

impl Metrics {
    /// Count one handled request.
    pub fn record_request(&self, method: &Method, status: StatusCode) {
        self.requests[method_index(method)][class_index(status)].fetch_add(1, Ordering::Relaxed);
    }

    /// Render the registry as Prometheus text exposition format (0.0.4).
    ///
    /// Every series is emitted even at zero: a stable set of series is what makes
    /// `rate()` and dashboards work across a restart.
    pub fn render(&self, snapshot: MetricsSnapshot) -> String {
        let mut out = String::with_capacity(4096);

        out.push_str(
            "# HELP karucode_http_requests_total HTTP requests handled, by method and status class.\n",
        );
        out.push_str("# TYPE karucode_http_requests_total counter\n");
        for (method_index, method) in METHODS.iter().enumerate() {
            for (class_index, class) in CLASSES.iter().enumerate() {
                let value = self.requests[method_index][class_index].load(Ordering::Relaxed);
                out.push_str(&format!(
                    "karucode_http_requests_total{{method=\"{method}\",status_class=\"{class}\"}} {value}\n"
                ));
            }
        }

        push_gauge(
            &mut out,
            "karucode_sse_connections_active",
            "Open server-sent-event connections.",
            self.sse_active.load(Ordering::Relaxed),
        );
        push_counter(
            &mut out,
            "karucode_sse_connections_total",
            "Server-sent-event connections accepted since process start.",
            self.sse_total.load(Ordering::Relaxed),
        );
        push_counter(
            &mut out,
            "karucode_sse_connections_rejected_total",
            "Server-sent-event connections refused because the cap was reached.",
            self.sse_rejected.load(Ordering::Relaxed),
        );
        push_gauge(
            &mut out,
            "karucode_runs_active",
            "Runs currently in flight.",
            snapshot.active_runs as u64,
        );
        push_counter(
            &mut out,
            "karucode_runs_started_total",
            "Runs started since process start.",
            kc_core::metrics::run_totals().0,
        );
        push_counter(
            &mut out,
            "karucode_runs_finished_total",
            "Runs that reached a terminal state since process start.",
            kc_core::metrics::run_totals().1,
        );
        push_gauge(
            &mut out,
            "karucode_uptime_seconds",
            "Process uptime in seconds.",
            snapshot.uptime_secs,
        );

        out.push_str("# HELP karucode_build_info Build metadata; the value is always 1.\n");
        out.push_str("# TYPE karucode_build_info gauge\n");
        out.push_str(&format!(
            "karucode_build_info{{version=\"{}\"}} 1\n",
            crate::VERSION
        ));
        out
    }
}

fn push_gauge(out: &mut String, name: &str, help: &str, value: u64) {
    out.push_str(&format!(
        "# HELP {name} {help}\n# TYPE {name} gauge\n{name} {value}\n"
    ));
}

fn push_counter(out: &mut String, name: &str, help: &str, value: u64) {
    out.push_str(&format!(
        "# HELP {name} {help}\n# TYPE {name} counter\n{name} {value}\n"
    ));
}

fn method_index(method: &Method) -> usize {
    METHODS
        .iter()
        .position(|known| *known == method.as_str())
        .unwrap_or(METHODS.len() - 1)
}

fn class_index(status: StatusCode) -> usize {
    match status.as_u16() / 100 {
        1 => 0,
        2 => 1,
        3 => 2,
        4 => 3,
        _ => 4,
    }
}

/// Count one request in the process-wide registry.
pub fn record_request(method: &Method, status: StatusCode) {
    registry().record_request(method, status);
}

/// Count a run that just started.
///
/// The runtime owns the run lifecycle, so the counter lives in `kc-core` (which
/// both crates depend on) and is incremented from `kc-runtime`. This is a thin
/// re-export so callers in this crate do not need to know that.
pub fn record_run_started() {
    kc_core::metrics::record_run_started();
}

/// Count a run that reached a terminal state.
pub fn record_run_finished() {
    kc_core::metrics::record_run_finished();
}

// ---------------------------------------------------------------------------
// SSE connection accounting
// ---------------------------------------------------------------------------

/// Holds one slot of the [`MAX_SSE_CONNECTIONS`] budget. Dropping it releases the
/// slot, so a client that vanishes without a clean shutdown still decrements the
/// gauge.
pub struct SseConnectionGuard {
    metrics: &'static Metrics,
}

impl Drop for SseConnectionGuard {
    fn drop(&mut self) {
        self.metrics.sse_active.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Take a connection slot, or `None` when the cap is already reached.
pub fn try_open_sse_connection() -> Option<SseConnectionGuard> {
    registry().try_open_sse()
}

impl Metrics {
    /// Take a slot from *this* registry. `&'static self` because the guard has to
    /// outlive the request and release the slot from `Drop`; tests use a leaked
    /// registry so they never touch the process-wide one.
    pub fn try_open_sse(&'static self) -> Option<SseConnectionGuard> {
        if acquire(&self.sse_active, MAX_SSE_CONNECTIONS as u64) {
            self.sse_total.fetch_add(1, Ordering::Relaxed);
            Some(SseConnectionGuard { metrics: self })
        } else {
            self.sse_rejected.fetch_add(1, Ordering::Relaxed);
            None
        }
    }
}

/// Reserve one unit of `counter` if that keeps it at or below `cap`.
///
/// A compare-exchange loop rather than `fetch_add` + rollback: two clients racing
/// for the last slot must not both win and then both back out.
fn acquire(counter: &AtomicU64, cap: u64) -> bool {
    let mut current = counter.load(Ordering::Acquire);
    loop {
        if current >= cap {
            return false;
        }
        match counter.compare_exchange_weak(
            current,
            current + 1,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => return true,
            Err(actual) => current = actual,
        }
    }
}

// ---------------------------------------------------------------------------
// Request ids
// ---------------------------------------------------------------------------

/// Mint a fresh v4-shaped UUID.
///
/// `uuid` is a workspace dependency but is only a *dev*-dependency of this crate,
/// so it is not available to `src/`. This is a real RFC 4122 version-4 layout
/// (version nibble and variant bits set) over a splitmix64 stream seeded from the
/// OS. A request id is a correlation handle, not a capability, so it does not
/// need to be a CSPRNG output - it needs to be unique and unguessable enough that
/// two concurrent requests never collide.
pub fn new_request_id() -> String {
    let hi = next_random_u64();
    let lo = next_random_u64();
    let hex = format!("{hi:016x}{lo:016x}");
    // The version nibble belongs to `time_hi_and_version` (hex index 12) and the
    // variant bits to `clock_seq_hi_and_reserved` (hex index 16) - not to the
    // leading nibble, which is the high bits of `time_low`.
    let mut digits: Vec<char> = hex.chars().collect();
    digits[12] = '4';
    digits[16] = match digits[16] {
        '0'..='3' => '8',
        '4'..='7' => '9',
        '8'..='b' => 'a',
        _ => 'b',
    };
    let hex: String = digits.into_iter().collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

fn next_random_u64() -> u64 {
    const GOLDEN: u64 = 0x9E37_79B9_7F4A_7C15;
    static STATE: OnceLock<AtomicU64> = OnceLock::new();
    let state = STATE.get_or_init(|| AtomicU64::new(random_seed()));
    let mut z = state
        .fetch_add(GOLDEN, Ordering::Relaxed)
        .wrapping_add(GOLDEN);
    // splitmix64 finaliser: cheap, and good enough that consecutive ids share no
    // visible structure.
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn random_seed() -> u64 {
    #[cfg(unix)]
    {
        use std::io::Read;
        if let Ok(mut file) = std::fs::File::open("/dev/urandom") {
            let mut bytes = [0u8; 8];
            if file.read_exact(&mut bytes).is_ok() {
                return u64::from_le_bytes(bytes);
            }
        }
    }
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos() as u64)
        .unwrap_or(0);
    nanos ^ ((std::process::id() as u64) << 32)
}

// ---------------------------------------------------------------------------
// Layers and the scrape handler
// ---------------------------------------------------------------------------

/// The outermost layer: mint a request id, time the request, log one line, count
/// it and echo the id back.
///
/// It sits outside the auth and `Host` guards on purpose - a `401` or `403` is
/// exactly the request an operator needs to see, and a layer inside the guards
/// would never run for them.
///
/// The span itself is owned by the `TraceLayer` just inside this one (see
/// [`request_span`]), so this layer only emits the access-log line with plain
/// fields. That keeps one span per request instead of two nested ones with the
/// same name.
pub async fn observability(request: Request<Body>, next: Next) -> Response {
    observe(request, next, registry()).await
}

/// [`observability`] against an explicit registry.
///
/// Split out so a test can drive the layer against a private registry: the
/// process-wide one is shared by every test in the binary, and tests run in
/// parallel, so asserting on a delta there is inherently racy.
async fn observe(mut request: Request<Body>, next: Next, metrics: &'static Metrics) -> Response {
    let request_id = new_request_id();
    let method = request.method().clone();
    // `path()`, never `query()`: `?token=` is a bearer credential.
    let path = request.uri().path().to_string();
    request
        .extensions_mut()
        .insert(RequestId(request_id.clone()));

    let started = Instant::now();
    let mut response = next.run(request).await;
    let latency = started.elapsed();
    let status = response.status();

    metrics.record_request(&method, status);
    tracing::info!(
        request_id = %request_id,
        method = %method,
        path = %path,
        status = status.as_u16(),
        latency_ms = latency.as_millis() as u64,
        "request"
    );

    if let Ok(value) = HeaderValue::from_str(&request_id) {
        response
            .headers_mut()
            .insert(HeaderName::from_static(REQUEST_ID_HEADER), value);
    }
    response
}

/// `TraceLayer::on_response`: stamp the outcome onto the request span.
///
/// The access-log line is emitted by [`observability`]; this only fills in the
/// fields that are unknown when the span opens, so a span exported to a tracing
/// backend carries its status and latency.
pub fn record_response(
    response: &Response<Body>,
    latency: std::time::Duration,
    span: &tracing::Span,
) {
    span.record("status", response.status().as_u16());
    span.record("latency_ms", latency.as_millis() as u64);
}

/// `TraceLayer` span factory: reuse the id [`observability`] already minted, or
/// mint one when the layer is used on its own (tests, embeddings).
pub fn request_span(request: &Request<Body>) -> tracing::Span {
    let request_id = request
        .extensions()
        .get::<RequestId>()
        .map(|id| id.0.clone())
        .unwrap_or_else(new_request_id);
    tracing::info_span!(
        "http.request",
        request_id = %request_id,
        method = %request.method(),
        path = %request.uri().path(),
        status = tracing::field::Empty,
        latency_ms = tracing::field::Empty,
    )
}

/// `GET /metrics` in Prometheus text exposition format.
///
/// Registered by the lead in `routes/mod.rs`; see the hand-off note in the change
/// description for the exact line.
pub async fn metrics(State(state): State<AppState>) -> impl IntoResponse {
    render_response(MetricsSnapshot {
        active_runs: state.runtime().active_runs().len(),
        uptime_secs: state.uptime_secs(),
    })
}

/// The scrape response, split out from the handler so its content type and body
/// are testable without building a whole `AppState`.
///
/// The content type is the versioned one Prometheus expects; a plain
/// `text/plain` makes some scrapers fall back to the legacy parser.
pub fn render_response(snapshot: MetricsSnapshot) -> Response {
    (
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/plain; version=0.0.4; charset=utf-8"),
        )],
        registry().render(snapshot),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::get;
    use axum::Router;
    use tower::ServiceExt;

    fn snapshot() -> MetricsSnapshot {
        MetricsSnapshot {
            active_runs: 2,
            uptime_secs: 42,
        }
    }

    #[test]
    fn the_scrape_renders_prometheus_text_with_the_expected_series() {
        let text = Metrics::default().render(snapshot());
        for expected in [
            "# TYPE karucode_http_requests_total counter",
            "karucode_http_requests_total{method=\"GET\",status_class=\"2xx\"} 0",
            "karucode_http_requests_total{method=\"POST\",status_class=\"5xx\"} 0",
            "# TYPE karucode_sse_connections_active gauge",
            "karucode_sse_connections_active 0",
            "# TYPE karucode_sse_connections_total counter",
            "# TYPE karucode_sse_connections_rejected_total counter",
            "# TYPE karucode_runs_active gauge",
            "karucode_runs_active 2",
            "# TYPE karucode_runs_started_total counter",
            "# TYPE karucode_runs_finished_total counter",
            "# TYPE karucode_uptime_seconds gauge",
            "karucode_uptime_seconds 42",
            "karucode_build_info{version=\"",
        ] {
            assert!(text.contains(expected), "missing `{expected}` in:\n{text}");
        }
        // Every non-comment line must be `name[{labels}] value`, which is what a
        // Prometheus parser requires; a stray blank or a missing value breaks the
        // whole scrape, not just one series.
        for line in text.lines().filter(|line| !line.starts_with('#')) {
            let (series, value) = line
                .rsplit_once(' ')
                .unwrap_or_else(|| panic!("no value in `{line}`"));
            assert!(!series.is_empty(), "empty series name in `{line}`");
            assert!(
                value.parse::<f64>().is_ok(),
                "`{value}` is not a number in `{line}`"
            );
        }
        assert!(
            text.ends_with('\n'),
            "the exposition must end with a newline"
        );
    }

    #[test]
    fn a_request_increments_the_counter_for_its_method_and_class() {
        let metrics = Metrics::default();
        metrics.record_request(&Method::GET, StatusCode::OK);
        metrics.record_request(&Method::GET, StatusCode::NOT_FOUND);
        metrics.record_request(&Method::POST, StatusCode::INTERNAL_SERVER_ERROR);
        // An unknown method must not grow the series set.
        metrics.record_request(&Method::from_bytes(b"BREW").unwrap(), StatusCode::OK);

        let text = metrics.render(snapshot());
        assert!(
            text.contains("karucode_http_requests_total{method=\"GET\",status_class=\"2xx\"} 1")
        );
        assert!(
            text.contains("karucode_http_requests_total{method=\"GET\",status_class=\"4xx\"} 1")
        );
        assert!(
            text.contains("karucode_http_requests_total{method=\"POST\",status_class=\"5xx\"} 1")
        );
        assert!(
            text.contains("karucode_http_requests_total{method=\"OTHER\",status_class=\"2xx\"} 1")
        );
        // Untouched series stay at zero rather than disappearing.
        assert!(
            text.contains("karucode_http_requests_total{method=\"DELETE\",status_class=\"2xx\"} 0")
        );
    }

    #[test]
    fn request_ids_are_unique_and_carry_the_v4_layout() {
        let first = new_request_id();
        let second = new_request_id();
        assert_ne!(first, second, "two requests must not share an id");
        assert_eq!(first.len(), 36, "not a canonical UUID: {first}");
        let groups: Vec<&str> = first.split('-').collect();
        assert_eq!(
            groups.iter().map(|g| g.len()).collect::<Vec<_>>(),
            vec![8, 4, 4, 4, 12],
            "not a canonical UUID: {first}"
        );
        assert!(
            groups[2].starts_with('4'),
            "version nibble is not 4: {first}"
        );
        assert!(
            matches!(groups[3].as_bytes()[0], b'8' | b'9' | b'a' | b'b'),
            "variant bits are not 10xx: {first}"
        );
        assert!(first.chars().all(|c| c == '-' || c.is_ascii_hexdigit()));
    }

    /// The header has to be on the response, and different on every request, or
    /// it cannot correlate anything.
    #[tokio::test]
    async fn the_layer_echoes_a_unique_request_id_on_every_response() {
        let app: Router = Router::new()
            .route("/ping", get(|| async { "pong" }))
            .layer(axum::middleware::from_fn(observability));

        let mut seen = Vec::new();
        for _ in 0..3 {
            let request = Request::builder().uri("/ping").body(Body::empty()).unwrap();
            let response = app.clone().oneshot(request).await.expect("router call");
            assert_eq!(response.status(), StatusCode::OK);
            let value = response
                .headers()
                .get(REQUEST_ID_HEADER)
                .expect("X-Request-Id is missing")
                .to_str()
                .expect("X-Request-Id is not ASCII")
                .to_string();
            assert_eq!(value.len(), 36, "not a UUID: {value}");
            seen.push(value);
        }
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), 3, "request ids repeated: {seen:?}");
    }

    /// A failing request is exactly the one an operator needs to correlate, so the
    /// id must survive an error response too.
    #[tokio::test]
    async fn an_error_response_still_carries_the_request_id() {
        let app: Router = Router::new()
            .route(
                "/boom",
                get(|| async { crate::error::ApiError::internal("boom") }),
            )
            .layer(axum::middleware::from_fn(observability));

        let request = Request::builder().uri("/boom").body(Body::empty()).unwrap();
        let response = app.oneshot(request).await.expect("router call");
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert!(response.headers().contains_key(REQUEST_ID_HEADER));
    }

    #[test]
    fn the_connection_cap_is_enforced() {
        let counter = AtomicU64::new(0);
        for _ in 0..4 {
            assert!(acquire(&counter, 4), "a slot below the cap was refused");
        }
        assert_eq!(counter.load(Ordering::Acquire), 4);
        assert!(!acquire(&counter, 4), "the cap was exceeded");
        assert_eq!(
            counter.load(Ordering::Acquire),
            4,
            "a refused acquire must not consume a slot"
        );
    }

    #[test]
    fn dropping_the_guard_releases_the_slot() {
        let before = registry().sse_active.load(Ordering::Acquire);
        let guard = try_open_sse_connection().expect("a slot below the cap");
        assert_eq!(registry().sse_active.load(Ordering::Acquire), before + 1);
        drop(guard);
        assert_eq!(
            registry().sse_active.load(Ordering::Acquire),
            before,
            "the gauge did not return to its previous value"
        );
    }

    /// The scrape endpoint has to answer with the versioned Prometheus content
    /// type and a body a parser accepts - that is the whole contract.
    #[tokio::test]
    async fn the_scrape_endpoint_returns_prometheus_text() {
        let response = render_response(snapshot());
        assert_eq!(response.status(), StatusCode::OK);
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .expect("no content type")
            .to_str()
            .expect("content type is not ASCII");
        assert!(
            content_type.starts_with("text/plain"),
            "unexpected content type: {content_type}"
        );
        assert!(
            content_type.contains("version=0.0.4"),
            "the versioned content type is what scrapers expect: {content_type}"
        );

        let body = axum::body::to_bytes(response.into_body(), 256 * 1024)
            .await
            .expect("body");
        let text = String::from_utf8(body.to_vec()).expect("body is not UTF-8");
        for expected in [
            "karucode_http_requests_total",
            "karucode_sse_connections_active",
            "karucode_runs_active",
            "karucode_runs_started_total",
            "karucode_runs_finished_total",
            "karucode_uptime_seconds",
        ] {
            assert!(text.contains(expected), "missing `{expected}` in:\n{text}");
        }
    }

    /// The counter the layer increments is the one the scrape reports, so a
    /// request is visible end to end.
    ///
    /// Driven against a private registry: the process-wide one is shared by every
    /// test in the binary and they run in parallel, so a delta there is racy.
    #[tokio::test]
    async fn a_request_through_the_layer_shows_up_in_the_scrape() {
        let metrics: &'static Metrics = Box::leak(Box::new(Metrics::default()));
        let app: Router = Router::new()
            .route("/ping", get(|| async { "pong" }))
            .layer(axum::middleware::from_fn(
                move |request: Request<Body>, next: Next| observe(request, next, metrics),
            ));

        let request = Request::builder().uri("/ping").body(Body::empty()).unwrap();
        let response = app.oneshot(request).await.expect("router call");
        assert_eq!(response.status(), StatusCode::OK);

        let counted = metrics.requests[method_index(&Method::GET)][class_index(StatusCode::OK)]
            .load(Ordering::Relaxed);
        assert_eq!(counted, 1, "the request was not counted");

        let text = metrics.render(snapshot());
        assert!(
            text.contains("karucode_http_requests_total{method=\"GET\",status_class=\"2xx\"} 1"),
            "the scrape does not report the counted request:\n{text}"
        );
    }
}
