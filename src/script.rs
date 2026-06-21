//! `execute_script` — synchronous JavaScript execution against the active
//! browsing context. thirtyfour returns a `ScriptRet` whose `.json()` is
//! the raw `serde_json::Value`; we hand that straight through to stryke
//! so scripts can read structured return values via `from_json`.

use anyhow::{anyhow, Result};
use serde_json::Value;

use crate::common::{block_on, get_element, resolve_session};

/// Run `script` with the given JSON `args` array. Each arg that is a
/// `{"__element__": <id>}` object is resolved to the corresponding
/// `WebElement` so JS can manipulate elements `Selenium::find` already
/// returned — same convention selenium-python's `execute_script` uses
/// when you pass a `WebElement` as an arg.
///
/// `WebElement` implements `Serialize` and emits the W3C-spec element
/// reference shape (`{"element-6066-11e4-a52e-4f735466cecf": "<id>"}`),
/// which the WebDriver server unmarshals back into a real DOM element on
/// the JS side. So once we resolve the stryke-side u64 to a `WebElement`,
/// the rest is plain `serde_json::to_value`.
pub fn execute_script(session: Option<u64>, script: String, args: Vec<Value>) -> Result<Value> {
    let drv = resolve_session(session)?;
    // Resolve element references BEFORE entering the async block so the
    // map error surfaces with the original element id intact.
    let mut prepared: Vec<Value> = Vec::with_capacity(args.len());
    for a in args {
        match element_ref_id(&a) {
            Some(id) => {
                let elem = get_element(id)?;
                prepared.push(
                    serde_json::to_value(&elem)
                        .map_err(|e| anyhow!("element serialize failed: {e}"))?,
                );
            }
            None => prepared.push(a),
        }
    }
    block_on(async move {
        let ret = drv
            .execute(&script, prepared)
            .await
            .map_err(|e| anyhow!("execute_script failed: {e}"))?;
        Ok(ret.json().clone())
    })
}

/// Run an asynchronous script. The script is handed a callback as its LAST
/// argument (`arguments[arguments.length - 1]`); the value it passes to that
/// callback becomes the return value, and the call blocks until the callback
/// fires or the session's script timeout (see `Selenium::set_script_timeout`)
/// elapses. Element references in `args` are resolved exactly as
/// `execute_script` does.
pub fn execute_async_script(
    session: Option<u64>,
    script: String,
    args: Vec<Value>,
) -> Result<Value> {
    let drv = resolve_session(session)?;
    let mut prepared: Vec<Value> = Vec::with_capacity(args.len());
    for a in args {
        match element_ref_id(&a) {
            Some(id) => {
                let elem = get_element(id)?;
                prepared.push(
                    serde_json::to_value(&elem)
                        .map_err(|e| anyhow!("element serialize failed: {e}"))?,
                );
            }
            None => prepared.push(a),
        }
    }
    block_on(async move {
        let ret = drv
            .execute_async(&script, prepared)
            .await
            .map_err(|e| anyhow!("execute_async_script failed: {e}"))?;
        Ok(ret.json().clone())
    })
}

/// Detect a stryke-side element reference: a JSON object of the form
/// `{"__element__": <id>}`. The double-underscore prefix matches Python's
/// "this isn't your business" naming convention and is exceptionally
/// unlikely to collide with a real script-argument key.
fn element_ref_id(v: &Value) -> Option<u64> {
    v.as_object()
        .and_then(|m| m.get("__element__"))
        .and_then(|x| x.as_u64())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn element_ref_recognized() {
        // The `__element__` key is the contract the .stk wrapper relies
        // on; renaming it = silently breaking every script that passes a
        // WebElement to execute_script.
        assert_eq!(element_ref_id(&json!({"__element__": 42})), Some(42));
    }

    #[test]
    fn element_ref_rejects_other_shapes() {
        assert_eq!(element_ref_id(&json!({})), None);
        assert_eq!(element_ref_id(&json!({"id": 42})), None);
        assert_eq!(element_ref_id(&json!(42)), None);
        assert_eq!(element_ref_id(&json!("__element__")), None);
        assert_eq!(element_ref_id(&json!({"__element__": "abc"})), None);
    }
}
