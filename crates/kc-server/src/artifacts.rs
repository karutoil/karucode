//! Chat artifact previews: the isolated document a transcript renders.
//!
//! An **artifact** is a fenced block in a chat message that the web UI draws as
//! live content instead of code (an HTML mock, a diagram, a table). Everything
//! that can be drawn without help is drawn in the browser by first-party
//! components, but a page cannot be: it needs its own document, its own origin
//! and its own policy, or its scripts would run with the app's authority.
//!
//! `srcdoc` cannot do the job, and this was measured rather than assumed: a
//! `srcdoc` frame inherits *this app's* CSP (`default-src 'self'`), then runs in
//! an opaque origin where `'self'` matches nothing, so its inline `<style>`, its
//! inline `<script>` and any relative asset are all refused and the preview comes
//! up nearly blank. `blob:` URLs inherit the same policy container for the same
//! reason. The answer is the one `routes::workspace::preview` already uses for
//! workspace files: serve a real URL with a route-local policy.
//!
//! So the browser POSTs the artifact's source, and gets back a URL it can point
//! an `<iframe>` at:
//!
//! ```text
//! POST /api/artifacts   {"html": "<!doctype html>…", "theme": "dark"}
//!   → 201 {"id": "9f2c…", "url": "/api/artifacts/9f2c…", "bytes": 1204, "theme": "dark"}
//! GET  /api/artifacts/9f2c…
//!   → 200 text/html  (sandboxed, opaque origin, no network)
//! ```
//!
//! # Why a store at all, and why content-addressed
//!
//! The document the frame loads has to be reachable by URL, and a URL has a
//! length limit (a 100 KB mock would not fit in one). So the bytes have to live
//! server-side for a while. They live in **memory only**, keyed by the SHA-256 of
//! the exact bytes served: re-posting the same artifact in the same theme returns
//! the same id, never a second copy, and the whole store is dropped when the
//! process exits. Nothing is written to the workspace or the database - a chat
//! artifact is a rendering, not a file the user asked for.
//!
//! Because it is a cache, a lost entry is not an error the reader has to see: the
//! client re-posts whatever it is about to display, so a reload after a restart
//! (or an eviction) simply stores it again.
//!
//! # The policy is deliberately stricter than a workspace preview
//!
//! [`crate::routes::workspace::PREVIEW_CONTENT_SECURITY_POLICY`] lets a page load
//! remote assets, which is right for *your own* `index.html` and wrong for text a
//! model wrote: `img-src *` in a prompt-injected mock is a tracking beacon (the
//! attacker learns you opened it) and a way to exfiltrate whatever the mock was
//! given. [`ARTIFACT_CONTENT_SECURITY_POLICY`] therefore keeps the opaque origin
//! and the inline script/style a mock needs, and closes the network entirely.
//!
//! # What a model should write
//!
//! The user-facing half of this (what to put in a fence, which kinds render) is
//! documented for the agent in the `web-ui` topic, and is stated in the system
//! prompt so an agent can use it without being asked.

use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

/// One artifact document may be this large. A mock is markup, not an asset
/// bundle; the ceiling is what keeps a single request from being a memory
/// problem, and it is reported to the caller as a documented `413`.
pub const MAX_ARTIFACT_BYTES: usize = 1024 * 1024;

/// The whole store's budget. Roughly [`MAX_ARTIFACT_BYTES`] mocks fit, and a
/// transcript never needs that many at once.
pub const MAX_STORE_BYTES: usize = 16 * 1024 * 1024;

/// Entries kept before the oldest is evicted.
pub const MAX_STORE_ENTRIES: usize = 128;

/// Hex characters of SHA-256 kept as the id. 128 bits: collision-free in
/// practice, and short enough to read in a log line.
pub const ID_LEN: usize = 32;

/// Policy for [`crate::routes::artifacts::get_artifact`]: a rendered artifact.
///
/// * `sandbox allow-scripts`, never `allow-same-origin`, is what puts the
///   document in an **opaque origin**. That is the whole security property: the
///   frame cannot read the app's `localStorage` (where the bearer token lives),
///   cannot call `/api` (it has no origin to be same-origin with), cannot register
///   a service worker and cannot navigate or frame the app.
/// * `script-src 'unsafe-inline' 'unsafe-eval'` and `style-src 'unsafe-inline'`
///   are what make a mock a mock: its own script and styles run. This is safe
///   *because* the origin is opaque - a hostile script can only reach the
///   document it was already given.
/// * `img-src data: blob:` and `font-src data:` allow an inline image or font,
///   and refuse every remote one. A mock is offline by design: a prompt-injected
///   `<img src="https://…">` must not become a beacon that tells a third party
///   the reader opened the message. (A mock that references a CDN stylesheet or a
///   web font renders without it, which is a degradation, not a break.)
/// * `connect-src 'none'` closes fetch/XHR/WebSocket - the one channel that could
///   lift data out of the document - and `form-action 'none'` closes submission.
/// * `base-uri 'none'` stops a `<base>` re-pointing the document's own URLs;
///   `frame-src`/`child-src`/`worker-src`/`object-src 'none'` stop it nesting or
///   loading plugins.
/// * `frame-ancestors 'self'` is narrower than the app-wide `'none'` so the
///   transcript's own frame may embed this response and nothing else can. The
///   `sandbox` directive also makes [`crate::app::security_headers`] withhold
///   `X-Frame-Options: DENY`, which would otherwise block that frame.
pub const ARTIFACT_CONTENT_SECURITY_POLICY: &str = "sandbox allow-scripts; default-src 'none'; \
     img-src data: blob:; media-src data: blob:; font-src data:; \
     style-src 'unsafe-inline'; script-src 'unsafe-inline' 'unsafe-eval' blob: data:; \
     connect-src 'none'; frame-src 'none'; child-src 'none'; worker-src 'none'; \
     object-src 'none'; form-action 'none'; base-uri 'none'; frame-ancestors 'self'";

/// Which way round the document's own defaults go.
///
/// A *fragment* is wrapped in a shell that has to paint a background and a text
/// colour, and those have to pair up or the mock is unreadable. The theme is the
/// app's own mode, so a dark transcript does not get a white rectangle, and the
/// author's own CSS still wins wherever it says something.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ArtifactTheme {
    #[default]
    Light,
    Dark,
}

impl ArtifactTheme {
    /// Parse the wire name (`light` / `dark`). Anything else is light: the
    /// default has to be readable for a mock written against white.
    pub fn parse(raw: Option<&str>) -> Self {
        match raw.map(str::to_ascii_lowercase).as_deref() {
            Some("dark") => Self::Dark,
            _ => Self::Light,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    /// `(color-scheme, background, foreground)` for the wrapper shell.
    fn colors(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::Light => ("light", "#ffffff", "#111827"),
            Self::Dark => ("dark", "#16181c", "#e7e9ee"),
        }
    }
}

/// The script every artifact document carries, so the card can size to it.
///
/// A frame has no intrinsic height, so without this a 60px mock sits in a 420px
/// white box. The document measures itself and tells the parent, which clamps the
/// height and grows or shrinks the frame. It is injected into a fragment's shell
/// and into a complete document alike (see [`inject_reporter`]) because the card
/// has no other way to learn the height of an opaque-origin document.
///
/// The measurement is the **content** height, not `documentElement.scrollHeight`:
/// that one is at least the viewport, so a frame that starts at 420 would report
/// 420 for ever and never shrink (measured in Chromium before this was fixed).
///
/// The parent accepts the message only from its own frame and only with this
/// `type`, and clamps whatever number arrives - a hostile mock can make its own
/// card tall, which is the whole extent of what it can do with this.
const RESIZE_REPORTER: &str = "<script>(function(){\
function post(){try{\
var body=document.body;\
var root=document.documentElement;\
var content=Math.max(\
body?body.scrollHeight:0,\
body?Math.ceil(body.getBoundingClientRect().height):0,\
root?Math.ceil(root.getBoundingClientRect().height):0);\
parent.postMessage({type:'karucode-artifact',height:content},'*')}catch(e){}}\
addEventListener('load',post);\
if(window.ResizeObserver){new ResizeObserver(post).observe(document.documentElement)}\
setTimeout(post,60);setTimeout(post,500);})();</script>";

/// Put [`RESIZE_REPORTER`] into a complete document the author wrote.
///
/// Appended before the closing `</body>` when there is one, else at the end. This
/// is the only edit ever made to an author's document, and it is what stops a
/// real page from rendering inside a fixed-height letterbox.
pub fn inject_reporter(document: &str) -> String {
    let lower = document.to_ascii_lowercase();
    if let Some(index) = lower.rfind("</body>") {
        let mut out = String::with_capacity(document.len() + RESIZE_REPORTER.len());
        out.push_str(&document[..index]);
        out.push_str(RESIZE_REPORTER);
        out.push_str(&document[index..]);
        return out;
    }
    let mut out = String::with_capacity(document.len() + RESIZE_REPORTER.len());
    out.push_str(document);
    out.push_str(RESIZE_REPORTER);
    out
}

/// The id of one document: 128 bits of SHA-256, hex.
pub fn artifact_id(document: &[u8]) -> String {
    let digest = Sha256::digest(document);
    let mut out = String::with_capacity(ID_LEN);
    for byte in digest.iter() {
        out.push_str(&format!("{byte:02x}"));
        if out.len() >= ID_LEN {
            break;
        }
    }
    out.truncate(ID_LEN);
    out
}

/// Is this a whole page rather than a fragment?
///
/// A complete document keeps its own `<head>`, charset, styles and (usually)
/// background: injecting a house style into it would be the renderer editing the
/// author's work. Only a fragment gets the shell. Both get the resize reporter -
/// that is a measurement, not a style.
pub fn is_complete_document(source: &str) -> bool {
    let head = source.trim_start();
    let lower = head[..head.len().min(400)].to_ascii_lowercase();
    lower.starts_with("<!doctype html") || lower.starts_with("<html") || lower.contains("<html ")
}

/// The bytes served for one artifact: the source, wrapped only if it is a
/// fragment.
pub fn wrap_document(source: &str, theme: ArtifactTheme) -> Vec<u8> {
    if is_complete_document(source) {
        return inject_reporter(source).into_bytes();
    }
    let (scheme, background, foreground) = theme.colors();
    let document = format!(
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<style>html{{color-scheme:{scheme};background:{background};color:{foreground}}}\
body{{margin:0;padding:16px;font:14px/1.55 system-ui,-apple-system,\"Segoe UI\",sans-serif}}\
img{{max-width:100%}}</style></head><body>\n{source}\n{RESIZE_REPORTER}\n</body></html>\n"
    );
    document.into_bytes()
}

struct Entry {
    body: Vec<u8>,
}

#[derive(Default)]
struct Inner {
    entries: HashMap<String, Entry>,
    /// Insertion order, oldest first. Kept beside the map so eviction does not
    /// have to scan it, and so nothing in this file can forget to update one.
    order: VecDeque<String>,
    bytes: usize,
}

/// The in-memory, content-addressed artifact document store.
#[derive(Default)]
pub struct ArtifactStore {
    inner: Mutex<Inner>,
}

impl ArtifactStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Store one document and return its id.
    ///
    /// Idempotent by content: the same bytes always map to the same entry, so a
    /// transcript that is re-rendered (or reloaded, or rendered twice by two
    /// clients) posts the same artifact as often as it likes without growing the
    /// store. A document larger than [`MAX_STORE_BYTES`] is not stored at all -
    /// the caller rejects it against [`MAX_ARTIFACT_BYTES`] long before this, and
    /// there is no sensible way to hold it.
    pub fn insert(&self, document: Vec<u8>) -> String {
        let id = artifact_id(&document);
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if inner.entries.contains_key(&id) {
            return id;
        }
        if document.len() > MAX_STORE_BYTES {
            return id;
        }
        inner.bytes += document.len();
        inner.order.push_back(id.clone());
        inner.entries.insert(id.clone(), Entry { body: document });
        while inner.entries.len() > MAX_STORE_ENTRIES
            || (inner.bytes > MAX_STORE_BYTES && inner.entries.len() > 1)
        {
            let Some(oldest) = inner.order.pop_front() else {
                break;
            };
            if let Some(removed) = inner.entries.remove(&oldest) {
                inner.bytes = inner.bytes.saturating_sub(removed.body.len());
            }
        }
        id
    }

    /// The bytes behind an id, if they are still here.
    pub fn get(&self, id: &str) -> Option<Vec<u8>> {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.entries.get(id).map(|entry| entry.body.clone())
    }

    /// `(entries, bytes)` held. Used by tests and by the health endpoint's
    /// accounting if it ever wants it.
    pub fn stats(&self) -> (usize, usize) {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        (inner.entries.len(), inner.bytes)
    }
}

/// Is this a well-formed id? Checked before the store is touched so a path like
/// `/api/artifacts/../../etc/passwd` never reaches a `HashMap` lookup with a
/// string that means something else.
pub fn valid_id(id: &str) -> bool {
    id.len() == ID_LEN && id.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifacts_the_same_bytes_map_to_one_entry() {
        let store = ArtifactStore::new();
        let document = wrap_document("<p>hi</p>", ArtifactTheme::Light);
        let first = store.insert(document.clone());
        let second = store.insert(document.clone());
        assert_eq!(first, second);
        assert_eq!(store.stats(), (1, document.len()));
        assert_eq!(store.get(&first).as_deref(), Some(document.as_slice()));
    }

    #[test]
    fn artifacts_differ_by_content_and_by_theme() {
        let store = ArtifactStore::new();
        let light = store.insert(wrap_document("<p>hi</p>", ArtifactTheme::Light));
        let dark = store.insert(wrap_document("<p>hi</p>", ArtifactTheme::Dark));
        let other = store.insert(wrap_document("<p>bye</p>", ArtifactTheme::Light));
        assert_ne!(light, dark, "the wrapper differs, so the document does");
        assert_ne!(light, other);
        assert_eq!(store.stats().0, 3);
    }

    #[test]
    fn artifacts_are_evicted_oldest_first_and_forget_their_bytes() {
        let store = ArtifactStore::new();
        let first = store.insert(wrap_document("<p>first</p>", ArtifactTheme::Light));
        for index in 0..MAX_STORE_ENTRIES {
            store.insert(wrap_document(&format!("<p>{index}</p>"), ArtifactTheme::Light));
        }
        let (entries, bytes) = store.stats();
        assert_eq!(entries, MAX_STORE_ENTRIES);
        assert!(bytes <= MAX_STORE_BYTES);
        assert!(store.get(&first).is_none(), "the oldest entry is the one evicted");
    }

    #[test]
    fn artifacts_reinserting_an_evicted_id_stores_it_again() {
        let store = ArtifactStore::new();
        let document = wrap_document("<p>again</p>", ArtifactTheme::Light);
        let id = store.insert(document.clone());
        for index in 0..MAX_STORE_ENTRIES {
            store.insert(wrap_document(&format!("<p>{index}</p>"), ArtifactTheme::Light));
        }
        assert!(store.get(&id).is_none());
        assert_eq!(store.insert(document.clone()), id);
        assert_eq!(store.get(&id).as_deref(), Some(document.as_slice()));
    }

    #[test]
    fn artifacts_ids_are_lowercase_hex_of_the_documented_length() {
        let id = artifact_id(b"<!doctype html><p>x</p>");
        assert_eq!(id.len(), ID_LEN);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(id.chars().all(|c| !c.is_ascii_uppercase()));
        assert!(valid_id(&id));
        assert!(!valid_id(""));
        assert!(!valid_id(&id.to_uppercase()));
        assert!(!valid_id(&id[..ID_LEN - 1]));
        assert!(!valid_id("../../etc/passwd"));
    }

    #[test]
    fn artifacts_wrap_a_fragment_into_a_readable_document() {
        let light = String::from_utf8(wrap_document("<button>Buy</button>", ArtifactTheme::Light))
            .expect("utf-8");
        assert!(light.starts_with("<!doctype html>"));
        assert!(light.contains("<meta charset=\"utf-8\">"));
        assert!(light.contains("color-scheme:light"));
        assert!(light.contains("background:#ffffff"));
        assert!(light.contains("<button>Buy</button>"));
        assert!(!light.contains("<html lang=\"en\">\n<html"), "one document, not two");
        assert!(light.contains("karucode-artifact"), "the card can size to it");

        let dark =
            String::from_utf8(wrap_document("<button>Buy</button>", ArtifactTheme::Dark)).unwrap();
        assert!(dark.contains("color-scheme:dark"));
        assert!(dark.contains("background:#16181c"));
        assert_ne!(light, dark);
    }

    #[test]
    fn artifacts_serve_a_complete_document_with_its_own_styles() {
        let source = "<!doctype html><html><head><style>b{color:red}</style></head>\
<body><b>hi</b></body></html>";
        let served = String::from_utf8(wrap_document(source, ArtifactTheme::Dark)).unwrap();
        assert!(served.contains("<style>b{color:red}</style>"), "the author's CSS survives");
        assert!(is_complete_document(source));
        assert!(is_complete_document("  <html lang=\"en\">"));
        assert!(!is_complete_document("<div>fragment</div>"));
        assert!(!is_complete_document("A sentence about <html> as a word"));
    }

    #[test]
    fn artifacts_inject_the_resize_reporter_inside_the_body() {
        let source = "<!doctype html><html><body><b>hi</b></body></html>";
        let served = inject_reporter(source);
        let reporter = served.find("karucode-artifact").expect("reporter present");
        let body = served.find("</body>").expect("body close present");
        assert!(reporter < body, "the reporter goes inside the body, before its close");
        assert!(served.ends_with("</body></html>"), "the closing tags are kept: {served}");

        // A document with no body close still gets it, at the end.
        let bare = inject_reporter("<p>hi</p>");
        assert!(bare.starts_with("<p>hi</p>"));
        assert!(bare.contains("karucode-artifact"));
    }

    #[test]
    fn artifacts_theme_parsing_defaults_to_light() {
        assert_eq!(ArtifactTheme::parse(Some("dark")), ArtifactTheme::Dark);
        assert_eq!(ArtifactTheme::parse(Some("DARK")), ArtifactTheme::Dark);
        assert_eq!(ArtifactTheme::parse(Some("light")), ArtifactTheme::Light);
        assert_eq!(ArtifactTheme::parse(Some("chartreuse")), ArtifactTheme::Light);
        assert_eq!(ArtifactTheme::parse(None), ArtifactTheme::Light);
        assert_eq!(ArtifactTheme::default(), ArtifactTheme::Light);
        assert_eq!(ArtifactTheme::Dark.as_str(), "dark");
    }

    #[test]
    fn artifacts_policy_keeps_the_origin_opaque_and_the_network_closed() {
        let policy = ARTIFACT_CONTENT_SECURITY_POLICY;
        assert!(policy.starts_with("sandbox allow-scripts"));
        assert!(!policy.contains("allow-same-origin"), "that would share the app's origin");
        assert!(policy.contains("connect-src 'none'"));
        // No wildcard sources at all: a remote asset must not load, so a
        // prompt-injected mock cannot beacon out.
        assert!(!policy.contains("*;") && !policy.contains("* "), "no wildcard source");
        assert!(!policy.contains("img-src *"));
    }
}
