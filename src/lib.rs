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
}
