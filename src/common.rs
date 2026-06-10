//! Shared helpers for the stryke-selenium cdylib service modules.
//!
//! Two perf wins versus the obvious sync-bridge / fresh-runtime-per-call shape:
//!
//! 1. A single `tokio::runtime::Runtime` is built once on first FFI entry and
//!    reused for every `block_on(...)`. Spinning up an MT-runtime is ~1 ms +
//!    one OS thread per worker — paying that per `Selenium::*` call would
//!    dominate the FFI dispatch time for cheap ops like `title()`.
//! 2. `WebDriver` (the entire Selenium session — browser process + WebDriver
//!    HTTP keep-alive pool) and `WebElement` handles live in process-global
//!    registries. Stryke scripts can open a browser once and drive it across
//!    many script lines without re-handshaking the session each call.
//!
//! Element handles use a monotonic `u64` id assigned client-side. The
//! WebDriver server has its own opaque element id; we never leak that to
//! stryke — the integer key is what gets serialized into JSON and threaded
//! through `Selenium::*` calls.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use anyhow::{anyhow, Result};
use once_cell::sync::OnceCell;
use thirtyfour::{WebDriver, WebElement};
use tokio::runtime::{Builder, Runtime};

/// Default WebDriver server URL — chromedriver's `--port=9515` default.
/// Users running a selenium-server-standalone JAR (4444) or geckodriver
/// (4444) pass an explicit `url` to `Selenium::open` to override.
pub const DEFAULT_WEBDRIVER_URL: &str = "http://localhost:9515";

/// Static set returned by `selenium__supported_browsers`. Members are the
/// strings the stryke caller passes as the `browser` arg to
/// `Selenium::open`; the driver code matches on these case-insensitively.
pub const SUPPORTED_BROWSERS: &[&str] = &["chrome", "firefox", "safari", "edge"];

/// Static set returned by `selenium__locator_strategies`. Members are the
/// `by` arg accepted by `Selenium::find` / `find_all` / `wait_for`.
pub const LOCATOR_STRATEGIES: &[&str] = &[
    "css",
    "id",
    "name",
    "xpath",
    "tag",
    "class",
    "link_text",
    "partial_link_text",
];

/// Lazily build a multi-threaded tokio runtime on first call and reuse it
/// for every subsequent `block_on(...)`. The runtime owns its worker
/// threads; they live for the rest of the stryke process. Building a fresh
/// runtime per FFI call would be ~1 ms of pure setup overhead — fine in
/// isolation, ruinous across thousands of `Selenium::*` calls in a script.
pub fn runtime() -> Result<&'static Runtime> {
    static RT: OnceCell<Runtime> = OnceCell::new();
    RT.get_or_try_init(|| {
        Builder::new_multi_thread()
            .enable_all()
            .thread_name("stryke-selenium")
            .build()
            .map_err(|e| anyhow!("tokio runtime init failed: {e}"))
    })
}

/// Run `fut` on the shared runtime and convert its `Result<T>` into the
/// `Result<T>` the FFI handler returns.
pub fn block_on<F, T>(fut: F) -> Result<T>
where
    F: std::future::Future<Output = Result<T>>,
{
    runtime()?.block_on(fut)
}

/// Active-session pointer. Selenium::* calls take an optional `session`
/// argument; when omitted, they default to whichever id is in here. The
/// first successful `Selenium::open` sets this; `Selenium::set_active`
/// overrides it.
static ACTIVE: AtomicU64 = AtomicU64::new(0);

pub fn get_active() -> Option<u64> {
    let v = ACTIVE.load(Ordering::SeqCst);
    if v == 0 {
        None
    } else {
        Some(v)
    }
}

pub fn set_active(id: u64) {
    ACTIVE.store(id, Ordering::SeqCst);
}

pub fn clear_active_if(id: u64) {
    let _ = ACTIVE.compare_exchange(id, 0, Ordering::SeqCst, Ordering::SeqCst);
}

/// Monotonic id generator shared by sessions + elements. Each registry
/// owns its own counter so a session id and an element id can collide
/// numerically — that's fine, they live in disjoint maps and are never
/// mixed in the stryke API.
fn next_id(counter: &AtomicU64) -> u64 {
    counter.fetch_add(1, Ordering::SeqCst) + 1
}

// ── session registry ────────────────────────────────────────────────────

fn sessions_map() -> &'static Mutex<HashMap<u64, WebDriver>> {
    static M: OnceCell<Mutex<HashMap<u64, WebDriver>>> = OnceCell::new();
    M.get_or_init(|| Mutex::new(HashMap::new()))
}

static SESSION_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Register a freshly opened `WebDriver`, return its client-side id, and
/// promote it to the active session if no other is active. Stryke scripts
/// that never juggle multiple browsers can ignore the id entirely.
pub fn register_session(driver: WebDriver) -> Result<u64> {
    let id = next_id(&SESSION_COUNTER);
    sessions_map()
        .lock()
        .map_err(|e| anyhow!("session registry poisoned: {e}"))?
        .insert(id, driver);
    if get_active().is_none() {
        set_active(id);
    }
    Ok(id)
}

/// Clone the `WebDriver` for `id`. thirtyfour's `WebDriver` is internally
/// an `Arc` around the HTTP client + session config, so cloning is cheap
/// and the returned handle shares the underlying session with the one
/// in the registry.
pub fn get_session(id: u64) -> Result<WebDriver> {
    sessions_map()
        .lock()
        .map_err(|e| anyhow!("session registry poisoned: {e}"))?
        .get(&id)
        .cloned()
        .ok_or_else(|| anyhow!("no Selenium session #{id} — open one with Selenium::open"))
}

/// Resolve an optional `id`: explicit ids win, else fall back to the
/// active-session pointer, else error with a hint pointing at
/// `Selenium::open`.
pub fn resolve_session(id: Option<u64>) -> Result<WebDriver> {
    let id = match id {
        Some(i) if i != 0 => i,
        _ => get_active()
            .ok_or_else(|| anyhow!("no active Selenium session — call Selenium::open first"))?,
    };
    get_session(id)
}

/// Remove a session from the registry and return it so the caller can
/// `quit()` it. Also clears the active pointer if this was the active one.
pub fn take_session(id: u64) -> Result<WebDriver> {
    let drv = sessions_map()
        .lock()
        .map_err(|e| anyhow!("session registry poisoned: {e}"))?
        .remove(&id)
        .ok_or_else(|| anyhow!("no Selenium session #{id}"))?;
    clear_active_if(id);
    Ok(drv)
}

/// Drain the registry, returning every session for shutdown. Used by
/// `Selenium::quit_all` to close every browser the script opened.
pub fn drain_sessions() -> Result<Vec<(u64, WebDriver)>> {
    let mut map = sessions_map()
        .lock()
        .map_err(|e| anyhow!("session registry poisoned: {e}"))?;
    let out: Vec<_> = map.drain().collect();
    ACTIVE.store(0, Ordering::SeqCst);
    Ok(out)
}

pub fn session_ids() -> Result<Vec<u64>> {
    let map = sessions_map()
        .lock()
        .map_err(|e| anyhow!("session registry poisoned: {e}"))?;
    let mut ids: Vec<u64> = map.keys().copied().collect();
    ids.sort_unstable();
    Ok(ids)
}

// ── element registry ────────────────────────────────────────────────────

fn elements_map() -> &'static Mutex<HashMap<u64, WebElement>> {
    static M: OnceCell<Mutex<HashMap<u64, WebElement>>> = OnceCell::new();
    M.get_or_init(|| Mutex::new(HashMap::new()))
}

static ELEMENT_COUNTER: AtomicU64 = AtomicU64::new(0);

pub fn register_element(elem: WebElement) -> Result<u64> {
    let id = next_id(&ELEMENT_COUNTER);
    elements_map()
        .lock()
        .map_err(|e| anyhow!("element registry poisoned: {e}"))?
        .insert(id, elem);
    Ok(id)
}

pub fn get_element(id: u64) -> Result<WebElement> {
    elements_map()
        .lock()
        .map_err(|e| anyhow!("element registry poisoned: {e}"))?
        .get(&id)
        .cloned()
        .ok_or_else(|| {
            anyhow!("no Selenium element #{id} (already dropped or never returned by find?)")
        })
}

pub fn drop_element(id: u64) -> Result<bool> {
    Ok(elements_map()
        .lock()
        .map_err(|e| anyhow!("element registry poisoned: {e}"))?
        .remove(&id)
        .is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_url_is_chromedriver_port() {
        // chromedriver's documented default is 9515. Test pins it so a
        // refactor doesn't silently change what `Selenium::open()` (no url)
        // connects to.
        assert_eq!(DEFAULT_WEBDRIVER_URL, "http://localhost:9515");
    }

    #[test]
    fn supported_browsers_contains_the_big_four() {
        // Every member is what `Selenium::open(browser => "...")` expects.
        // The stryke .stk side has no allowlist of its own — this is THE
        // source of truth.
        for b in ["chrome", "firefox", "safari", "edge"] {
            assert!(
                SUPPORTED_BROWSERS.contains(&b),
                "{b} dropped from SUPPORTED_BROWSERS"
            );
        }
    }

    #[test]
    fn locator_strategies_match_thirtyfour_by_variants() {
        // Members map 1:1 to `By::*` arms in `driver::by_from()`.
        // Drift between this table and that match arm = silent
        // "unsupported locator" error path.
        for s in [
            "css",
            "id",
            "name",
            "xpath",
            "tag",
            "class",
            "link_text",
            "partial_link_text",
        ] {
            assert!(LOCATOR_STRATEGIES.contains(&s), "{s} not registered");
        }
    }

    #[test]
    fn active_session_pointer_round_trips() {
        // No external setup — exercises only the AtomicU64 helpers, not
        // the live registry, so it's safe to run unattended in CI.
        set_active(42);
        assert_eq!(get_active(), Some(42));
        clear_active_if(7); // wrong id — should not clear
        assert_eq!(get_active(), Some(42));
        clear_active_if(42);
        assert_eq!(get_active(), None);
    }
}
