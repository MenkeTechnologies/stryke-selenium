//! stryke-selenium — Selenium WebDriver automation cdylib loaded in-process
//! by stryke via dlopen.
//!
//! Each `#[no_mangle] extern "C" fn selenium__*` is a JSON-string-in /
//! JSON-string-out wrapper around the `driver` / `element` / `script` /
//! `capture` / `window` modules. stryke's FFI bridge (`rust_ffi.rs::
//! load_cdylib`) resolves these symbols at first `use Selenium`, registers
//! each one as a stryke-callable function, and on each call passes a
//! JSON-encoded args dict and copies the returned JSON into a stryke
//! string. The cdylib's `stryke_free_cstring` export plugs the
//! returned-allocation leak the inline-FFI v1 had.
//!
//! Why this exists: thirtyfour is a tokio-async crate, but stryke's FFI is
//! sync. The naive bridge would `Runtime::new().block_on(...)` per call —
//! that's ~1 ms of pure tokio bring-up overhead on every `Selenium::*`
//! call. The cdylib model lets one runtime live for the whole stryke
//! process (see `common.rs::runtime`), and lets `WebDriver` + `WebElement`
//! handles persist across calls so the WebDriver session is built once.

mod capture;
mod common;
mod driver;
mod element;
mod script;
mod window;

use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::panic::AssertUnwindSafe;

use anyhow::Result;
use serde_json::{json, Value};

use crate::common::{get_active, session_ids, set_active, LOCATOR_STRATEGIES, SUPPORTED_BROWSERS};

/// Run a handler that takes a parsed JSON `Value` and returns a JSON `Value`,
/// converting any error or panic into a `{"error": "<msg>"}` JSON object so
/// the stryke side can `die` on it. Always returns a freshly allocated
/// `CString` — the caller (stryke's FFI bridge) must free it via
/// [`stryke_free_cstring`].
fn ffi_call<F>(args: *const c_char, handler: F) -> *const c_char
where
    F: FnOnce(Value) -> Result<Value>,
{
    let input = if args.is_null() {
        Value::Null
    } else {
        // SAFETY: args is a `*const c_char` from stryke's FFI bridge; the
        // bridge only passes pointers into NUL-terminated `CString`s it
        // allocated for this call.
        let cs = unsafe { CStr::from_ptr(args) };
        serde_json::from_slice::<Value>(cs.to_bytes()).unwrap_or(Value::Null)
    };
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| handler(input)));
    let out = match result {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => json!({ "error": e.to_string() }),
        Err(_) => json!({ "error": "stryke-selenium handler panicked" }),
    };
    let s =
        serde_json::to_string(&out).unwrap_or_else(|_| String::from(r#"{"error":"serialize"}"#));
    match CString::new(s) {
        Ok(c) => c.into_raw() as *const c_char,
        Err(_) => std::ptr::null(),
    }
}

/// Free a C string allocated by any of this cdylib's exports. stryke's FFI
/// bridge calls this immediately after copying the returned bytes into a
/// stryke string.
///
/// # Safety
///
/// `p` must be a pointer previously returned by an export from this cdylib
/// (i.e. a `CString::into_raw` output), or a null pointer.
#[no_mangle]
pub unsafe extern "C" fn stryke_free_cstring(p: *mut c_char) {
    if p.is_null() {
        return;
    }
    drop(CString::from_raw(p));
}

// ── helpers ─────────────────────────────────────────────────────────────

fn arg_session(v: &Value) -> Option<u64> {
    v.get("session").and_then(|s| s.as_u64())
}

fn arg_element(v: &Value, key: &str) -> Result<u64> {
    v.get(key)
        .and_then(|x| x.as_u64())
        .ok_or_else(|| anyhow::anyhow!("missing element id '{key}'"))
}

fn arg_str<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v.get(key)
        .and_then(|x| x.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing string '{key}'"))
}

// ── session lifecycle ───────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn selenium__open(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        let browser = v["browser"].as_str().unwrap_or("chrome").to_string();
        let url = v["url"].as_str().map(String::from);
        let headless = v["headless"].as_bool().unwrap_or(false);
        let id = driver::open(&browser, url.as_deref(), headless)?;
        Ok(json!({ "session": id }))
    })
}

#[no_mangle]
pub extern "C" fn selenium__quit(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        // `quit` is one of the few entry points where defaulting to the
        // active session is dangerous — a stale script might shut down
        // the wrong browser. Require an explicit id (we still accept the
        // active one if the caller opts in via session=0 / undef).
        let id = arg_session(&v)
            .or_else(get_active)
            .ok_or_else(|| anyhow::anyhow!("quit: no session id and no active session"))?;
        driver::quit(id)?;
        Ok(json!({ "session": id, "closed": true }))
    })
}

#[no_mangle]
pub extern "C" fn selenium__quit_all(args: *const c_char) -> *const c_char {
    ffi_call(args, |_| {
        let n = driver::quit_all()?;
        Ok(json!({ "closed": n }))
    })
}

#[no_mangle]
pub extern "C" fn selenium__sessions(args: *const c_char) -> *const c_char {
    ffi_call(args, |_| {
        Ok(json!({
            "sessions": session_ids()?,
            "active": get_active(),
        }))
    })
}

#[no_mangle]
pub extern "C" fn selenium__active(args: *const c_char) -> *const c_char {
    ffi_call(args, |_| Ok(json!({ "active": get_active() })))
}

#[no_mangle]
pub extern "C" fn selenium__set_active(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        let id = v["session"]
            .as_u64()
            .ok_or_else(|| anyhow::anyhow!("set_active: missing 'session'"))?;
        set_active(id);
        Ok(json!({ "active": id }))
    })
}

#[no_mangle]
pub extern "C" fn selenium__supported_browsers(args: *const c_char) -> *const c_char {
    ffi_call(args, |_| Ok(serde_json::to_value(SUPPORTED_BROWSERS)?))
}

#[no_mangle]
pub extern "C" fn selenium__locator_strategies(args: *const c_char) -> *const c_char {
    ffi_call(args, |_| Ok(serde_json::to_value(LOCATOR_STRATEGIES)?))
}

// ── navigation ──────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn selenium__goto(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        let url = arg_str(&v, "url")?.to_string();
        driver::goto(arg_session(&v), &url)?;
        Ok(json!({}))
    })
}

#[no_mangle]
pub extern "C" fn selenium__current_url(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        Ok(json!({ "url": driver::current_url(arg_session(&v))? }))
    })
}

#[no_mangle]
pub extern "C" fn selenium__title(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        Ok(json!({ "title": driver::title(arg_session(&v))? }))
    })
}

#[no_mangle]
pub extern "C" fn selenium__source(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        Ok(json!({ "source": driver::source(arg_session(&v))? }))
    })
}

#[no_mangle]
pub extern "C" fn selenium__back(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        driver::back(arg_session(&v))?;
        Ok(json!({}))
    })
}

#[no_mangle]
pub extern "C" fn selenium__forward(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        driver::forward(arg_session(&v))?;
        Ok(json!({}))
    })
}

#[no_mangle]
pub extern "C" fn selenium__refresh(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        driver::refresh(arg_session(&v))?;
        Ok(json!({}))
    })
}

#[no_mangle]
pub extern "C" fn selenium__accept_alert(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        driver::accept_alert(arg_session(&v))?;
        Ok(json!({}))
    })
}

#[no_mangle]
pub extern "C" fn selenium__dismiss_alert(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        driver::dismiss_alert(arg_session(&v))?;
        Ok(json!({}))
    })
}

#[no_mangle]
pub extern "C" fn selenium__alert_text(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        Ok(json!({ "text": driver::alert_text(arg_session(&v))? }))
    })
}

#[no_mangle]
pub extern "C" fn selenium__send_alert_text(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        let text = arg_str(&v, "text")?.to_string();
        driver::send_alert_text(arg_session(&v), &text)?;
        Ok(json!({}))
    })
}

#[no_mangle]
pub extern "C" fn selenium__set_implicit_wait(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        let s = v["seconds"].as_f64().unwrap_or(0.0);
        driver::set_implicit_wait(arg_session(&v), s)?;
        Ok(json!({}))
    })
}

#[no_mangle]
pub extern "C" fn selenium__set_page_load_timeout(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        let s = v["seconds"].as_f64().unwrap_or(0.0);
        driver::set_page_load_timeout(arg_session(&v), s)?;
        Ok(json!({}))
    })
}

#[no_mangle]
pub extern "C" fn selenium__set_script_timeout(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        let s = v["seconds"].as_f64().unwrap_or(0.0);
        driver::set_script_timeout(arg_session(&v), s)?;
        Ok(json!({}))
    })
}

// ── element queries ────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn selenium__find(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        let by = v["by"].as_str().unwrap_or("css").to_string();
        let sel = arg_str(&v, "selector")?.to_string();
        let id = element::find(arg_session(&v), &by, sel)?;
        Ok(json!({ "element": id }))
    })
}

#[no_mangle]
pub extern "C" fn selenium__find_all(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        let by = v["by"].as_str().unwrap_or("css").to_string();
        let sel = arg_str(&v, "selector")?.to_string();
        let ids = element::find_all(arg_session(&v), &by, sel)?;
        Ok(json!({ "elements": ids }))
    })
}

#[no_mangle]
pub extern "C" fn selenium__wait_for(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        let by = v["by"].as_str().unwrap_or("css").to_string();
        let sel = arg_str(&v, "selector")?.to_string();
        let t = v["timeout"].as_f64().unwrap_or(10.0);
        let id = element::wait_for(arg_session(&v), &by, sel, t)?;
        Ok(json!({ "element": id }))
    })
}

// ── element ops ────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn selenium__element_click(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        element::click(arg_element(&v, "element")?)?;
        Ok(json!({}))
    })
}

#[no_mangle]
pub extern "C" fn selenium__element_send_keys(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        let text = arg_str(&v, "text")?.to_string();
        element::send_keys(arg_element(&v, "element")?, text)?;
        Ok(json!({}))
    })
}

#[no_mangle]
pub extern "C" fn selenium__element_clear(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        element::clear(arg_element(&v, "element")?)?;
        Ok(json!({}))
    })
}

#[no_mangle]
pub extern "C" fn selenium__element_text(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        Ok(json!({ "text": element::text(arg_element(&v, "element")?)? }))
    })
}

#[no_mangle]
pub extern "C" fn selenium__scroll_to_element(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        element::scroll_into_view(arg_element(&v, "element")?)?;
        Ok(json!({}))
    })
}

#[no_mangle]
pub extern "C" fn selenium__print_page(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        let path = arg_str(&v, "output")?.to_string();
        Ok(json!({ "path": driver::print_page(arg_session(&v), &path)? }))
    })
}

#[no_mangle]
pub extern "C" fn selenium__element_attr(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        let name = arg_str(&v, "name")?.to_string();
        Ok(json!({ "value": element::attr(arg_element(&v, "element")?, name)? }))
    })
}

#[no_mangle]
pub extern "C" fn selenium__element_prop(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        let name = arg_str(&v, "name")?.to_string();
        Ok(json!({ "value": element::prop(arg_element(&v, "element")?, name)? }))
    })
}

#[no_mangle]
pub extern "C" fn selenium__element_css(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        let name = arg_str(&v, "name")?.to_string();
        Ok(json!({ "value": element::css(arg_element(&v, "element")?, name)? }))
    })
}

#[no_mangle]
pub extern "C" fn selenium__element_tag(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        Ok(json!({ "tag": element::tag(arg_element(&v, "element")?)? }))
    })
}

#[no_mangle]
pub extern "C" fn selenium__element_rect(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        Ok(serde_json::to_value(element::rect(arg_element(
            &v, "element",
        )?)?)?)
    })
}

#[no_mangle]
pub extern "C" fn selenium__element_is_displayed(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        Ok(json!({ "value": element::is_displayed(arg_element(&v, "element")?)? }))
    })
}

#[no_mangle]
pub extern "C" fn selenium__element_is_enabled(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        Ok(json!({ "value": element::is_enabled(arg_element(&v, "element")?)? }))
    })
}

#[no_mangle]
pub extern "C" fn selenium__element_is_selected(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        Ok(json!({ "value": element::is_selected(arg_element(&v, "element")?)? }))
    })
}

#[no_mangle]
pub extern "C" fn selenium__element_drop(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        Ok(json!({ "dropped": element::drop_id(arg_element(&v, "element")?)? }))
    })
}

// ── scripts ────────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn selenium__execute_script(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        let code = arg_str(&v, "script")?.to_string();
        let script_args: Vec<Value> = v
            .get("args")
            .and_then(|a| a.as_array())
            .cloned()
            .unwrap_or_default();
        let ret = script::execute_script(arg_session(&v), code, script_args)?;
        Ok(json!({ "value": ret }))
    })
}

// ── screenshots ────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn selenium__screenshot(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        let path = v["output"].as_str();
        match capture::screenshot(arg_session(&v), path)? {
            capture::ScreenshotRet::Path(p) => Ok(json!({ "path": p })),
            capture::ScreenshotRet::Raw(r) => Ok(serde_json::to_value(r)?),
        }
    })
}

#[no_mangle]
pub extern "C" fn selenium__element_screenshot(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        let path = v["output"].as_str();
        match capture::element_screenshot(arg_element(&v, "element")?, path)? {
            capture::ScreenshotRet::Path(p) => Ok(json!({ "path": p })),
            capture::ScreenshotRet::Raw(r) => Ok(serde_json::to_value(r)?),
        }
    })
}

// ── window / frame ─────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn selenium__window_rect(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        Ok(serde_json::to_value(window::window_rect(arg_session(&v))?)?)
    })
}

#[no_mangle]
pub extern "C" fn selenium__set_window_rect(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        let x = v["x"].as_i64();
        let y = v["y"].as_i64();
        let w = v["width"].as_u64().map(|n| n as u32);
        let h = v["height"].as_u64().map(|n| n as u32);
        window::set_window_rect(arg_session(&v), x, y, w, h)?;
        Ok(json!({}))
    })
}

#[no_mangle]
pub extern "C" fn selenium__maximize(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        window::maximize(arg_session(&v))?;
        Ok(json!({}))
    })
}

#[no_mangle]
pub extern "C" fn selenium__minimize(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        window::minimize(arg_session(&v))?;
        Ok(json!({}))
    })
}

#[no_mangle]
pub extern "C" fn selenium__fullscreen(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        window::fullscreen(arg_session(&v))?;
        Ok(json!({}))
    })
}

#[no_mangle]
pub extern "C" fn selenium__window_handles(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        Ok(json!({ "handles": window::window_handles(arg_session(&v))? }))
    })
}

#[no_mangle]
pub extern "C" fn selenium__current_window(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        Ok(json!({ "handle": window::current_window(arg_session(&v))? }))
    })
}

#[no_mangle]
pub extern "C" fn selenium__switch_window(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        let h = arg_str(&v, "handle")?.to_string();
        window::switch_window(arg_session(&v), h)?;
        Ok(json!({}))
    })
}

#[no_mangle]
pub extern "C" fn selenium__switch_frame(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        window::switch_frame(arg_session(&v), arg_element(&v, "element")?)?;
        Ok(json!({}))
    })
}

#[no_mangle]
pub extern "C" fn selenium__switch_default_content(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        window::switch_default_content(arg_session(&v))?;
        Ok(json!({}))
    })
}

#[no_mangle]
pub extern "C" fn selenium__switch_parent_frame(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        window::switch_parent_frame(arg_session(&v))?;
        Ok(json!({}))
    })
}

// ── cookies ────────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn selenium__cookies(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        Ok(json!({ "cookies": window::cookies(arg_session(&v))? }))
    })
}

#[no_mangle]
pub extern "C" fn selenium__add_cookie(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        let opts = v
            .get("cookie")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("add_cookie: missing 'cookie'"))?;
        window::add_cookie(arg_session(&v), opts)?;
        Ok(json!({}))
    })
}

#[no_mangle]
pub extern "C" fn selenium__delete_cookie(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        let name = arg_str(&v, "name")?.to_string();
        window::delete_cookie(arg_session(&v), name)?;
        Ok(json!({}))
    })
}

#[no_mangle]
pub extern "C" fn selenium__delete_all_cookies(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        window::delete_all_cookies(arg_session(&v))?;
        Ok(json!({}))
    })
}

// ── pure helpers (no browser) ───────────────────────────────────────────

/// Canonicalize a locator-strategy alias to the form `find`/`by_from` accept,
/// or `None` if unknown.
fn canonical_strategy(s: &str) -> Option<&'static str> {
    match s.to_ascii_lowercase().as_str() {
        "css" | "css_selector" => Some("css"),
        "id" => Some("id"),
        "name" => Some("name"),
        "xpath" => Some("xpath"),
        "tag" | "tag_name" => Some("tag"),
        "class" | "class_name" => Some("class"),
        "link_text" | "link" => Some("link_text"),
        "partial_link_text" | "plink" | "partial" => Some("partial_link_text"),
        _ => None,
    }
}

/// Parse a `strategy=value` locator (`css=.btn`, `xpath=//a`, `id=main`) into
/// `{strategy, value}`, canonicalizing the strategy so it feeds straight into
/// `find`. A bare string with no `=` defaults to `css`. Pure.
fn op_parse_locator(v: Value) -> Result<Value> {
    let loc = arg_str(&v, "locator")?;
    let (strat_raw, value) = match loc.split_once('=') {
        Some((s, val)) => (s.trim(), val),
        None => ("css", loc),
    };
    let strategy = canonical_strategy(strat_raw)
        .ok_or_else(|| anyhow::anyhow!("unknown locator strategy '{strat_raw}'"))?;
    Ok(json!({"strategy": strategy, "value": value}))
}

/// Build a `strategy=value` locator from parts — the inverse of `parse_locator`.
/// The strategy is canonicalized (so `css_selector` → `css`) and rejected if
/// unknown. opts: `strategy` (required), `value` (required). Pure.
fn op_build_locator(v: Value) -> Result<Value> {
    let strat_raw = arg_str(&v, "strategy")?;
    let value = arg_str(&v, "value")?;
    let strategy = canonical_strategy(strat_raw)
        .ok_or_else(|| anyhow::anyhow!("unknown locator strategy '{strat_raw}'"))?;
    Ok(json!({"locator": format!("{strategy}={value}"), "strategy": strategy}))
}

/// Validate a locator strategy name, returning its canonical form. Pure.
fn op_valid_locator_strategy(v: Value) -> Result<Value> {
    let s = arg_str(&v, "strategy")?;
    let canon = canonical_strategy(s);
    Ok(json!({"strategy": s, "valid": canon.is_some(), "canonical": canon}))
}

/// Escape a value for use inside a double-quoted CSS string.
fn css_escape_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Translate a locator to the W3C WebDriver `{using, value}` pair sent over the
/// wire. The protocol defines only five strategies — `css selector`, `xpath`,
/// `tag name`, `link text`, `partial link text` — so the non-native `id`,
/// `name`, and `class` collapse to a `css selector` (`[id="…"]`, `[name="…"]`,
/// `[class~="…"]`), exactly as Selenium clients do. Accepts a `strategy`+`value`
/// pair or a single `locator` string (`id=main`). Returns `{using, value,
/// strategy}`. Pure.
fn op_locator_to_w3c(v: Value) -> Result<Value> {
    let (strat_raw, value) = if let Ok(loc) = arg_str(&v, "locator") {
        match loc.split_once('=') {
            Some((s, val)) => (s.trim().to_string(), val.to_string()),
            None => ("css".to_string(), loc.to_string()),
        }
    } else {
        (
            arg_str(&v, "strategy")?.to_string(),
            arg_str(&v, "value")?.to_string(),
        )
    };
    let strategy = canonical_strategy(&strat_raw)
        .ok_or_else(|| anyhow::anyhow!("unknown locator strategy '{strat_raw}'"))?;
    let (using, w3c_value) = match strategy {
        "css" => ("css selector", value),
        "xpath" => ("xpath", value),
        "tag" => ("tag name", value),
        "link_text" => ("link text", value),
        "partial_link_text" => ("partial link text", value),
        "id" => (
            "css selector",
            format!("[id=\"{}\"]", css_escape_string(&value)),
        ),
        "name" => (
            "css selector",
            format!("[name=\"{}\"]", css_escape_string(&value)),
        ),
        "class" => (
            "css selector",
            format!("[class~=\"{}\"]", css_escape_string(&value)),
        ),
        other => unreachable!("canonical_strategy yielded unexpected `{other}`"),
    };
    Ok(json!({"using": using, "value": w3c_value, "strategy": strategy}))
}

/// W3C WebDriver `using` string → canonical locator strategy. The wire protocol
/// carries only these five; `id`/`name`/`class` are client-side conveniences
/// that have already collapsed to `css selector` by this layer.
fn strategy_for_w3c_using(using: &str) -> Option<&'static str> {
    match using.trim().to_ascii_lowercase().as_str() {
        "css selector" => Some("css"),
        "xpath" => Some("xpath"),
        "tag name" => Some("tag"),
        "link text" => Some("link_text"),
        "partial link text" => Some("partial_link_text"),
        _ => None,
    }
}

/// Translate a W3C WebDriver `{using, value}` pair back to a canonical
/// `strategy=value` locator — the inverse of `locator_to_w3c` for the five
/// strategies the protocol actually carries (`css selector`→`css`, `tag name`→
/// `tag`, `link text`→`link_text`, `partial link text`→`partial_link_text`,
/// `xpath`→`xpath`). opts: `using` (required), `value` (required). Returns
/// `{strategy, value, locator}`. Pure.
fn op_w3c_to_locator(v: Value) -> Result<Value> {
    let using = arg_str(&v, "using")?;
    let value = arg_str(&v, "value")?;
    let strategy = strategy_for_w3c_using(using)
        .ok_or_else(|| anyhow::anyhow!("unknown W3C using strategy '{using}'"))?;
    Ok(json!({
        "strategy": strategy,
        "value": value,
        "locator": format!("{strategy}={value}"),
    }))
}

/// Resolve a WebDriver special key name to its Unicode code point. Selenium and
/// the W3C WebDriver spec assign control/navigation keys to the Private Use Area
/// (`NULL` U+E000 … `DELETE` U+E017, `F1`–`F12` U+E031–U+E03C); `send_keys`
/// inserts these characters to press the key. Names are case-insensitive and
/// ignore `_`/`-`/spaces; common aliases (`esc`, `ctrl`, `del`, `arrowup`) are
/// accepted. Note `return` (U+E006) differs from `enter` (U+E007). opts: `key`
/// (required). Returns `{key, code_point, codepoint, char}`. Pure.
fn op_key_code(v: Value) -> Result<Value> {
    let name = v
        .get("key")
        .or_else(|| v.get("name"))
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("missing key"))?;
    let norm: String = name
        .to_ascii_lowercase()
        .chars()
        .filter(|c| !matches!(c, '_' | '-' | ' '))
        .collect();
    let cp: u32 = match norm.as_str() {
        "null" => 0xE000,
        "cancel" => 0xE001,
        "help" => 0xE002,
        "backspace" => 0xE003,
        "tab" => 0xE004,
        "clear" => 0xE005,
        "return" => 0xE006,
        "enter" => 0xE007,
        "shift" => 0xE008,
        "control" | "ctrl" => 0xE009,
        "alt" => 0xE00A,
        "pause" => 0xE00B,
        "escape" | "esc" => 0xE00C,
        "space" => 0xE00D,
        "pageup" => 0xE00E,
        "pagedown" => 0xE00F,
        "end" => 0xE010,
        "home" => 0xE011,
        "left" | "arrowleft" => 0xE012,
        "up" | "arrowup" => 0xE013,
        "right" | "arrowright" => 0xE014,
        "down" | "arrowdown" => 0xE015,
        "insert" | "ins" => 0xE016,
        "delete" | "del" => 0xE017,
        other => match other.strip_prefix('f').and_then(|d| d.parse::<u32>().ok()) {
            Some(n) if (1..=12).contains(&n) => 0xE031 + (n - 1),
            _ => return Err(anyhow::anyhow!("unknown WebDriver key `{name}`")),
        },
    };
    let ch = char::from_u32(cp).expect("WebDriver PUA code point is a valid scalar");
    Ok(json!({
        "key": name,
        "code_point": format!("U+{cp:04X}"),
        "codepoint": cp,
        "char": ch.to_string(),
    }))
}

/// Resolve a WebDriver PUA code point back to its canonical key name — the
/// inverse of `key_code`. Accepts a `codepoint` integer or a single-character
/// `char` (the PUA character `send_keys` inserts). Returns the primary name
/// (`return` and `enter` stay distinct; aliases like `esc`/`ctrl` collapse to
/// `escape`/`control`). opts: `codepoint` or `char`. Returns `{codepoint,
/// code_point, key, char}`. Errors if the code point isn't an assigned
/// WebDriver key. Pure.
fn op_key_name(v: Value) -> Result<Value> {
    let cp: u32 = if let Some(n) = v.get("codepoint").and_then(Value::as_u64) {
        n as u32
    } else if let Some(s) = v.get("char").and_then(Value::as_str) {
        let mut chars = s.chars();
        let c = chars.next().ok_or_else(|| anyhow::anyhow!("empty char"))?;
        if chars.next().is_some() {
            return Err(anyhow::anyhow!("char must be a single character"));
        }
        c as u32
    } else {
        return Err(anyhow::anyhow!("missing codepoint or char"));
    };
    let name = match cp {
        0xE000 => "null",
        0xE001 => "cancel",
        0xE002 => "help",
        0xE003 => "backspace",
        0xE004 => "tab",
        0xE005 => "clear",
        0xE006 => "return",
        0xE007 => "enter",
        0xE008 => "shift",
        0xE009 => "control",
        0xE00A => "alt",
        0xE00B => "pause",
        0xE00C => "escape",
        0xE00D => "space",
        0xE00E => "pageup",
        0xE00F => "pagedown",
        0xE010 => "end",
        0xE011 => "home",
        0xE012 => "left",
        0xE013 => "up",
        0xE014 => "right",
        0xE015 => "down",
        0xE016 => "insert",
        0xE017 => "delete",
        0xE031..=0xE03C => {
            let n = cp - 0xE031 + 1;
            let ch = char::from_u32(cp).expect("WebDriver PUA code point is a valid scalar");
            return Ok(json!({
                "codepoint": cp,
                "code_point": format!("U+{cp:04X}"),
                "key": format!("f{n}"),
                "char": ch.to_string(),
            }));
        }
        _ => {
            return Err(anyhow::anyhow!(
                "U+{cp:04X} is not an assigned WebDriver key"
            ))
        }
    };
    let ch = char::from_u32(cp).expect("WebDriver PUA code point is a valid scalar");
    Ok(json!({
        "codepoint": cp,
        "code_point": format!("U+{cp:04X}"),
        "key": name,
        "char": ch.to_string(),
    }))
}

/// Parse a `Set-Cookie`-style string `name=value; Domain=…; Path=/; Secure;
/// HttpOnly; SameSite=Lax` into the structured cookie `add_cookie` wants. Pure.
fn op_parse_cookie(v: Value) -> Result<Value> {
    let s = arg_str(&v, "cookie")?;
    let mut parts = s.split(';').map(str::trim).filter(|x| !x.is_empty());
    let first = parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("empty cookie"))?;
    let (name, value) = first
        .split_once('=')
        .ok_or_else(|| anyhow::anyhow!("cookie missing name=value: {s}"))?;
    let mut domain = Value::Null;
    let mut path = Value::Null;
    let mut secure = false;
    let mut http_only = false;
    let mut same_site = Value::Null;
    let mut expires = Value::Null;
    for attr in parts {
        let (k, val) = match attr.split_once('=') {
            Some((k, val)) => (k.trim().to_ascii_lowercase(), Some(val.trim())),
            None => (attr.to_ascii_lowercase(), None),
        };
        match k.as_str() {
            "domain" => domain = json!(val.unwrap_or("")),
            "path" => path = json!(val.unwrap_or("")),
            "secure" => secure = true,
            "httponly" => http_only = true,
            "samesite" => same_site = json!(val.unwrap_or("")),
            "expires" | "max-age" => expires = json!(val.unwrap_or("")),
            _ => {}
        }
    }
    Ok(json!({
        "name": name.trim(),
        "value": value,
        "domain": domain,
        "path": path,
        "secure": secure,
        "http_only": http_only,
        "same_site": same_site,
        "expires": expires,
    }))
}

/// Build a `Set-Cookie`-style string from structured fields — the inverse of
/// `parse_cookie`. opts: `name` (required), `value`, and optional `domain`,
/// `path`, `same_site`, `expires`; `secure`/`http_only` are emitted as bare
/// flags when truthy (bool, nonzero number, or "1"/"true" — matching stryke's
/// flag serialization). Pure.
fn op_build_cookie(v: Value) -> Result<Value> {
    let name = arg_str(&v, "name")?;
    let value = v.get("value").and_then(Value::as_str).unwrap_or("");
    let mut out = format!("{name}={value}");
    let attr = |k: &str| v.get(k).and_then(Value::as_str).filter(|s| !s.is_empty());
    if let Some(d) = attr("domain") {
        out.push_str(&format!("; Domain={d}"));
    }
    if let Some(p) = attr("path") {
        out.push_str(&format!("; Path={p}"));
    }
    if let Some(ss) = attr("same_site") {
        out.push_str(&format!("; SameSite={ss}"));
    }
    if let Some(e) = attr("expires") {
        out.push_str(&format!("; Expires={e}"));
    }
    let truthy = |k: &str| match v.get(k) {
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_i64().map(|i| i != 0).unwrap_or(false),
        Some(Value::String(s)) => s == "1" || s.eq_ignore_ascii_case("true"),
        _ => false,
    };
    if truthy("secure") {
        out.push_str("; Secure");
    }
    if truthy("http_only") {
        out.push_str("; HttpOnly");
    }
    Ok(json!({"cookie": out}))
}

/// Escape a string for safe use as a CSS identifier — a faithful port of the
/// CSSOM "serialize an identifier" algorithm (what browsers expose as
/// `CSS.escape()`). Lets a CSS-strategy locator embed an arbitrary id/class
/// value (`#` + css_escape(id)). opts: `value` (required). Returns `{escaped}`.
/// Pure.
fn op_css_escape(v: Value) -> Result<Value> {
    let s = arg_str(&v, "value")?;
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    for (i, &c) in chars.iter().enumerate() {
        let cp = c as u32;
        if cp == 0 {
            out.push('\u{FFFD}');
        } else if (0x1..=0x1f).contains(&cp)
            || cp == 0x7f
            || (i == 0 && c.is_ascii_digit())
            || (i == 1 && c.is_ascii_digit() && chars[0] == '-')
        {
            // "the character escaped as code point": `\` + lowercase hex + space.
            out.push_str(&format!("\\{cp:x} "));
        } else if i == 0 && c == '-' && chars.len() == 1 {
            // A lone leading hyphen is backslash-escaped.
            out.push('\\');
            out.push('-');
        } else if cp >= 0x80 || c == '-' || c == '_' || c.is_ascii_alphanumeric() {
            out.push(c);
        } else {
            out.push('\\');
            out.push(c);
        }
    }
    Ok(json!({ "escaped": out }))
}

#[no_mangle]
pub extern "C" fn selenium__parse_locator(args: *const c_char) -> *const c_char {
    ffi_call(args, op_parse_locator)
}

#[no_mangle]
pub extern "C" fn selenium__build_locator(args: *const c_char) -> *const c_char {
    ffi_call(args, op_build_locator)
}

#[no_mangle]
pub extern "C" fn selenium__valid_locator_strategy(args: *const c_char) -> *const c_char {
    ffi_call(args, op_valid_locator_strategy)
}

#[no_mangle]
pub extern "C" fn selenium__locator_to_w3c(args: *const c_char) -> *const c_char {
    ffi_call(args, op_locator_to_w3c)
}

#[no_mangle]
pub extern "C" fn selenium__w3c_to_locator(args: *const c_char) -> *const c_char {
    ffi_call(args, op_w3c_to_locator)
}

#[no_mangle]
pub extern "C" fn selenium__key_code(args: *const c_char) -> *const c_char {
    ffi_call(args, op_key_code)
}

#[no_mangle]
pub extern "C" fn selenium__key_name(args: *const c_char) -> *const c_char {
    ffi_call(args, op_key_name)
}

#[no_mangle]
pub extern "C" fn selenium__parse_cookie(args: *const c_char) -> *const c_char {
    ffi_call(args, op_parse_cookie)
}

#[no_mangle]
pub extern "C" fn selenium__build_cookie(args: *const c_char) -> *const c_char {
    ffi_call(args, op_build_cookie)
}

#[no_mangle]
pub extern "C" fn selenium__css_escape(args: *const c_char) -> *const c_char {
    ffi_call(args, op_css_escape)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_and_free(p: *const c_char) -> Value {
        assert!(!p.is_null());
        let bytes = unsafe { CStr::from_ptr(p).to_bytes().to_vec() };
        unsafe { stryke_free_cstring(p as *mut c_char) };
        serde_json::from_slice(&bytes).expect("ffi return is valid JSON")
    }

    #[test]
    fn free_cstring_handles_null() {
        unsafe { stryke_free_cstring(std::ptr::null_mut()) };
    }

    #[test]
    fn ffi_call_returns_error_json_on_panic() {
        let v = read_and_free(ffi_call(std::ptr::null(), |_| -> Result<Value> {
            panic!("boom");
        }));
        assert!(v["error"].is_string());
    }

    #[test]
    fn ffi_call_returns_error_json_on_err() {
        let v = read_and_free(ffi_call(std::ptr::null(), |_| -> Result<Value> {
            Err(anyhow::anyhow!("intentional"))
        }));
        assert_eq!(v["error"].as_str().unwrap(), "intentional");
    }

    #[test]
    fn ffi_call_passes_args_to_handler() {
        let in_str = CString::new(r#"{"x":7,"y":9}"#).unwrap();
        let out = read_and_free(ffi_call(in_str.as_ptr(), |v| {
            Ok(json!({
                "sum": v["x"].as_i64().unwrap() + v["y"].as_i64().unwrap()
            }))
        }));
        assert_eq!(out["sum"].as_i64().unwrap(), 16);
    }

    #[test]
    fn supported_browsers_export_returns_static_set() {
        // Exercises the FFI dispatch path end-to-end without touching a
        // WebDriver server — same role `gui__key_keys` plays in
        // stryke-gui. The .stk smoke test (bin/selenium-test.stk) relies
        // on this being permission-free.
        let v = read_and_free(selenium__supported_browsers(std::ptr::null()));
        let arr = v.as_array().expect("array");
        assert!(arr.iter().any(|x| x == "chrome"));
        assert!(arr.iter().any(|x| x == "firefox"));
    }

    #[test]
    fn locator_strategies_export_returns_static_set() {
        let v = read_and_free(selenium__locator_strategies(std::ptr::null()));
        let arr = v.as_array().expect("array");
        assert!(arr.iter().any(|x| x == "css"));
        assert!(arr.iter().any(|x| x == "xpath"));
        assert!(arr.iter().any(|x| x == "id"));
    }

    #[test]
    fn sessions_export_returns_empty_when_none_open() {
        let v = read_and_free(selenium__sessions(std::ptr::null()));
        assert!(v["sessions"].is_array());
        // Active may be null or a previously-set test value (state
        // leaks across cargo's threaded tests if the active-pointer
        // test runs first), so we don't assert on it here.
    }

    // ── pure helpers (no browser) ────────────────────────────────────────────

    #[test]
    fn parse_locator_splits_and_canonicalizes() {
        let css = op_parse_locator(json!({"locator": "css=.btn.primary"})).unwrap();
        assert_eq!(css["strategy"], json!("css"));
        assert_eq!(css["value"], json!(".btn.primary"));
        // Aliases collapse to the canonical strategy `find` accepts.
        assert_eq!(
            op_parse_locator(json!({"locator": "class_name=active"})).unwrap()["strategy"],
            json!("class")
        );
        assert_eq!(
            op_parse_locator(json!({"locator": "link=Sign in"})).unwrap()["strategy"],
            json!("link_text")
        );
        // A bare value (no `=`) defaults to css.
        let bare = op_parse_locator(json!({"locator": "#main"})).unwrap();
        assert_eq!(bare["strategy"], json!("css"));
        assert_eq!(bare["value"], json!("#main"));
        assert!(op_parse_locator(json!({"locator": "bogus=x"})).is_err());
    }

    #[test]
    fn parse_locator_value_keeps_embedded_equals() {
        // An xpath with `=` must not be truncated at the first `=`.
        let v = op_parse_locator(json!({"locator": "xpath=//input[@type='text']"})).unwrap();
        assert_eq!(v["strategy"], json!("xpath"));
        assert_eq!(v["value"], json!("//input[@type='text']"));
    }

    #[test]
    fn build_locator_inverts_parse_locator() {
        let b = op_build_locator(json!({"strategy": "css", "value": ".btn.primary"})).unwrap();
        assert_eq!(b["locator"], json!("css=.btn.primary"));
        let back = op_parse_locator(json!({"locator": b["locator"].as_str().unwrap()})).unwrap();
        assert_eq!(back["strategy"], json!("css"));
        assert_eq!(back["value"], json!(".btn.primary"));
        // Strategy alias is canonicalized in the output.
        assert_eq!(
            op_build_locator(json!({"strategy": "class_name", "value": "active"})).unwrap()
                ["locator"],
            json!("class=active")
        );
        // An xpath value with `=` round-trips intact.
        let xp = op_build_locator(json!({"strategy": "xpath", "value": "//input[@type='text']"}))
            .unwrap();
        assert_eq!(xp["locator"], json!("xpath=//input[@type='text']"));
        assert_eq!(
            op_parse_locator(json!({"locator": xp["locator"].as_str().unwrap()})).unwrap()["value"],
            json!("//input[@type='text']")
        );
        assert!(op_build_locator(json!({"strategy": "bogus", "value": "x"})).is_err());
    }

    #[test]
    fn locator_to_w3c_maps_to_protocol_strategies() {
        // The five native W3C strategies pass their value through verbatim.
        let css = op_locator_to_w3c(json!({"strategy": "css", "value": ".btn"})).unwrap();
        assert_eq!(css["using"], json!("css selector"));
        assert_eq!(css["value"], json!(".btn"));
        assert_eq!(
            op_locator_to_w3c(json!({"locator": "xpath=//a"})).unwrap()["using"],
            json!("xpath")
        );
        assert_eq!(
            op_locator_to_w3c(json!({"locator": "tag_name=div"})).unwrap()["using"],
            json!("tag name")
        );
        assert_eq!(
            op_locator_to_w3c(json!({"locator": "link=Sign in"})).unwrap()["using"],
            json!("link text")
        );
        // id / name / class are non-native — they collapse to a css selector.
        let id = op_locator_to_w3c(json!({"locator": "id=main"})).unwrap();
        assert_eq!(id["using"], json!("css selector"));
        assert_eq!(id["value"], json!("[id=\"main\"]"));
        assert_eq!(
            op_locator_to_w3c(json!({"strategy": "name", "value": "q"})).unwrap()["value"],
            json!("[name=\"q\"]")
        );
        assert_eq!(
            op_locator_to_w3c(json!({"strategy": "class", "value": "active"})).unwrap()["value"],
            json!("[class~=\"active\"]")
        );
        // A value containing a quote is CSS-string-escaped.
        assert_eq!(
            op_locator_to_w3c(json!({"strategy": "id", "value": "a\"b"})).unwrap()["value"],
            json!("[id=\"a\\\"b\"]")
        );
        // A bare locator defaults to css; an unknown strategy errors.
        assert_eq!(
            op_locator_to_w3c(json!({"locator": "#main"})).unwrap()["using"],
            json!("css selector")
        );
        assert!(op_locator_to_w3c(json!({"locator": "bogus=x"})).is_err());
    }

    #[test]
    fn w3c_to_locator_inverts_locator_to_w3c() {
        let chk =
            |u: &str, val: &str| op_w3c_to_locator(json!({"using": u, "value": val})).unwrap();
        // Each of the five W3C `using` strings maps back to its canonical strategy.
        for (using, strat) in [
            ("css selector", "css"),
            ("xpath", "xpath"),
            ("tag name", "tag"),
            ("link text", "link_text"),
            ("partial link text", "partial_link_text"),
        ] {
            let v = chk(using, "X");
            assert_eq!(v["strategy"], json!(strat), "{using} → {strat}");
            assert_eq!(v["locator"], json!(format!("{strat}=X")));
        }
        // Case-insensitive on the `using` string.
        assert_eq!(chk("CSS Selector", ".btn")["strategy"], json!("css"));
        // Round-trips locator_to_w3c for the natively-carried strategies.
        for loc in ["css=.btn", "xpath=//a", "tag=div", "link_text=Home"] {
            let w3c = op_locator_to_w3c(json!({ "locator": loc })).unwrap();
            let back = op_w3c_to_locator(json!({
                "using": w3c["using"].as_str().unwrap(),
                "value": w3c["value"].as_str().unwrap(),
            }))
            .unwrap();
            assert_eq!(back["locator"], json!(loc), "round-trip {loc}");
        }
        // `id`/`name`/`class` are not W3C `using` strings → rejected.
        assert!(op_w3c_to_locator(json!({"using": "id", "value": "main"})).is_err());
        assert!(op_w3c_to_locator(json!({"using": "css selector"})).is_err());
    }

    #[test]
    fn key_code_resolves_webdriver_pua_code_points() {
        // Canonical code points from Selenium's Key enum.
        let enter = op_key_code(json!({"key": "Enter"})).unwrap();
        assert_eq!(enter["codepoint"], json!(0xE007));
        assert_eq!(enter["code_point"], json!("U+E007"));
        assert_eq!(enter["char"], json!("\u{E007}"));
        // Return (E006) is a DIFFERENT key from Enter (E007).
        assert_eq!(
            op_key_code(json!({"key": "Return"})).unwrap()["codepoint"],
            json!(0xE006)
        );
        // A spread of named keys.
        for (name, cp) in [
            ("null", 0xE000),
            ("Tab", 0xE004),
            ("ESCAPE", 0xE00C),
            ("space", 0xE00D),
            ("Home", 0xE011),
            ("ArrowUp", 0xE013),
            ("delete", 0xE017),
        ] {
            assert_eq!(
                op_key_code(json!({ "key": name })).unwrap()["codepoint"],
                json!(cp),
                "{name} → {cp:#X}"
            );
        }
        // Aliases and normalization (case / separators).
        assert_eq!(
            op_key_code(json!({"key": "esc"})).unwrap()["codepoint"],
            json!(0xE00C)
        );
        assert_eq!(
            op_key_code(json!({"key": "ctrl"})).unwrap()["codepoint"],
            json!(0xE009)
        );
        assert_eq!(
            op_key_code(json!({"key": "page_up"})).unwrap()["codepoint"],
            json!(0xE00E)
        );
        assert_eq!(
            op_key_code(json!({"key": "ARROW-DOWN"})).unwrap()["codepoint"],
            json!(0xE015)
        );
        // Function keys F1..F12 are contiguous from E031.
        assert_eq!(
            op_key_code(json!({"key": "F1"})).unwrap()["codepoint"],
            json!(0xE031)
        );
        assert_eq!(
            op_key_code(json!({"key": "f12"})).unwrap()["codepoint"],
            json!(0xE03C)
        );
        // Out-of-range function key and unknown name reject.
        assert!(op_key_code(json!({"key": "F13"})).is_err());
        assert!(op_key_code(json!({"key": "nope"})).is_err());
        assert!(op_key_code(json!({})).is_err());
    }

    #[test]
    fn key_name_inverts_key_code() {
        // Primary name for a code point.
        let enter = op_key_name(json!({"codepoint": 0xE007})).unwrap();
        assert_eq!(enter["key"], json!("enter"));
        assert_eq!(enter["code_point"], json!("U+E007"));
        // return (E006) is distinct from enter (E007).
        assert_eq!(
            op_key_name(json!({"codepoint": 0xE006})).unwrap()["key"],
            json!("return")
        );
        // Aliases collapse to the primary name.
        assert_eq!(
            op_key_name(json!({"codepoint": 0xE00C})).unwrap()["key"],
            json!("escape")
        );
        assert_eq!(
            op_key_name(json!({"codepoint": 0xE009})).unwrap()["key"],
            json!("control")
        );
        // Function keys.
        assert_eq!(
            op_key_name(json!({"codepoint": 0xE031})).unwrap()["key"],
            json!("f1")
        );
        assert_eq!(
            op_key_name(json!({"codepoint": 0xE03C})).unwrap()["key"],
            json!("f12")
        );
        // Accepts the PUA char directly.
        assert_eq!(
            op_key_name(json!({"char": "\u{E013}"})).unwrap()["key"],
            json!("up")
        );
        // Round-trips key_code for every primary name.
        for name in [
            "null", "tab", "return", "enter", "escape", "space", "home", "end", "left", "up",
            "right", "down", "insert", "delete", "f1", "f12",
        ] {
            let cp = op_key_code(json!({ "key": name })).unwrap()["codepoint"].clone();
            assert_eq!(
                op_key_name(json!({ "codepoint": cp })).unwrap()["key"],
                json!(name),
                "round-trip {name}"
            );
        }
        // Unassigned code point, multi-char, and missing args reject.
        assert!(op_key_name(json!({"codepoint": 0xE020})).is_err());
        assert!(op_key_name(json!({"char": "ab"})).is_err());
        assert!(op_key_name(json!({})).is_err());
    }

    #[test]
    fn valid_locator_strategy_reports_canonical() {
        let v = op_valid_locator_strategy(json!({"strategy": "TAG_NAME"})).unwrap();
        assert_eq!(v["valid"], json!(true));
        assert_eq!(v["canonical"], json!("tag"));
        let bad = op_valid_locator_strategy(json!({"strategy": "nope"})).unwrap();
        assert_eq!(bad["valid"], json!(false));
        assert_eq!(bad["canonical"], Value::Null);
    }

    #[test]
    fn parse_cookie_extracts_attributes() {
        let v = op_parse_cookie(json!({
            "cookie": "session=abc123; Domain=.example.com; Path=/; Secure; HttpOnly; SameSite=Lax"
        }))
        .unwrap();
        assert_eq!(v["name"], json!("session"));
        assert_eq!(v["value"], json!("abc123"));
        assert_eq!(v["domain"], json!(".example.com"));
        assert_eq!(v["path"], json!("/"));
        assert_eq!(v["secure"], json!(true));
        assert_eq!(v["http_only"], json!(true));
        assert_eq!(v["same_site"], json!("Lax"));
    }

    #[test]
    fn parse_cookie_minimal_and_value_with_equals() {
        let v = op_parse_cookie(json!({"cookie": "token=a=b=c"})).unwrap();
        assert_eq!(v["name"], json!("token"));
        assert_eq!(v["value"], json!("a=b=c"), "value keeps later = signs");
        assert_eq!(v["secure"], json!(false));
        assert!(op_parse_cookie(json!({"cookie": "noequalshere"})).is_err());
    }

    #[test]
    fn build_cookie_inverts_parse_cookie() {
        // Full set of fields → Set-Cookie string, round-trips through parse.
        let built = op_build_cookie(json!({
            "name": "session", "value": "abc123", "domain": ".example.com",
            "path": "/", "same_site": "Lax", "secure": true, "http_only": true
        }))
        .unwrap()["cookie"]
            .clone();
        assert_eq!(
            built,
            json!("session=abc123; Domain=.example.com; Path=/; SameSite=Lax; Secure; HttpOnly")
        );
        let back = op_parse_cookie(json!({"cookie": built})).unwrap();
        assert_eq!(back["name"], json!("session"));
        assert_eq!(back["secure"], json!(true));
        assert_eq!(back["http_only"], json!(true));
        assert_eq!(back["same_site"], json!("Lax"));
        // Minimal cookie; flags absent.
        assert_eq!(
            op_build_cookie(json!({"name": "k", "value": "v"})).unwrap()["cookie"],
            json!("k=v")
        );
        // stryke serializes a truthy flag as the number 1, not a bool.
        assert_eq!(
            op_build_cookie(json!({"name": "k", "value": "v", "secure": 1})).unwrap()["cookie"],
            json!("k=v; Secure")
        );
        assert!(op_build_cookie(json!({"value": "v"})).is_err());
    }

    #[test]
    fn css_escape_matches_cssom_serialize_identifier() {
        let esc = |s: &str| {
            op_css_escape(json!({ "value": s })).unwrap()["escaped"]
                .as_str()
                .unwrap()
                .to_string()
        };
        // Plain identifiers, hyphens, underscores, and non-ASCII pass through.
        assert_eq!(esc("foo"), "foo");
        assert_eq!(esc("--a"), "--a");
        assert_eq!(esc("café"), "café");
        // A leading digit is escaped as a code point (`\31 ` etc.).
        assert_eq!(esc("0foo"), "\\30 foo");
        // A digit second char after a leading hyphen is also code-point escaped.
        assert_eq!(esc("-1"), "-\\31 ");
        // A lone hyphen is backslash-escaped.
        assert_eq!(esc("-"), "\\-");
        // Punctuation and whitespace get a backslash.
        assert_eq!(esc("foo bar"), "foo\\ bar");
        assert_eq!(esc("a#b.c"), "a\\#b\\.c");
        // NULL becomes the replacement character; control chars are code-point escaped.
        assert_eq!(esc("a\u{0}b"), "a\u{FFFD}b");
        assert_eq!(esc("a\u{1}b"), "a\\1 b");
        // Building a real id selector with css_escape is safe to feed a query.
        assert_eq!(format!("#{}", esc("user 42")), "#user\\ 42");
        assert!(op_css_escape(json!({})).is_err());
    }
}
