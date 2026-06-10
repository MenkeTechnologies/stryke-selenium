//! Window / frame / cookie operations.
//!
//! Cookies cross the FFI as raw JSON: `get_all_cookies` is serialized via
//! `serde_json::to_value` (thirtyfour's `Cookie` is the `cookie` crate's
//! `Cookie` with `serde` derive), and `add_cookie` is the inverse
//! `from_value`. This keeps the stryke side honest — it sees the same
//! field names the WebDriver protocol uses — and insulates us from
//! cookie-crate API drift across thirtyfour versions.

use anyhow::{anyhow, Result};
use serde::Serialize;
use serde_json::Value;
use thirtyfour::prelude::*;

use crate::common::{block_on, get_element, resolve_session};

#[derive(Serialize)]
pub struct WindowRect {
    pub x: i64,
    pub y: i64,
    pub width: u32,
    pub height: u32,
}

pub fn window_rect(session: Option<u64>) -> Result<WindowRect> {
    let drv = resolve_session(session)?;
    block_on(async move {
        let r = drv
            .get_window_rect()
            .await
            .map_err(|e| anyhow!("window_rect failed: {e}"))?;
        Ok(WindowRect {
            x: r.x,
            y: r.y,
            width: r.width as u32,
            height: r.height as u32,
        })
    })
}

pub fn set_window_rect(
    session: Option<u64>,
    x: Option<i64>,
    y: Option<i64>,
    width: Option<u32>,
    height: Option<u32>,
) -> Result<()> {
    let drv = resolve_session(session)?;
    block_on(async move {
        // WebDriver accepts a partial rect; we mirror that by reading the
        // current rect and overriding only the fields the caller set.
        let cur = drv
            .get_window_rect()
            .await
            .map_err(|e| anyhow!("set_window_rect: read current failed: {e}"))?;
        let nx = x.unwrap_or(cur.x);
        let ny = y.unwrap_or(cur.y);
        let nw = width.unwrap_or(cur.width as u32);
        let nh = height.unwrap_or(cur.height as u32);
        drv.set_window_rect(nx, ny, nw, nh)
            .await
            .map_err(|e| anyhow!("set_window_rect failed: {e}"))
    })
}

pub fn window_handles(session: Option<u64>) -> Result<Vec<String>> {
    let drv = resolve_session(session)?;
    block_on(async move {
        let hs = drv
            .windows()
            .await
            .map_err(|e| anyhow!("window_handles failed: {e}"))?;
        Ok(hs.into_iter().map(|h| h.to_string()).collect())
    })
}

pub fn current_window(session: Option<u64>) -> Result<String> {
    let drv = resolve_session(session)?;
    block_on(async move {
        let h = drv
            .window()
            .await
            .map_err(|e| anyhow!("current_window failed: {e}"))?;
        Ok(h.to_string())
    })
}

pub fn switch_window(session: Option<u64>, handle: String) -> Result<()> {
    let drv = resolve_session(session)?;
    block_on(async move {
        let hh = WindowHandle::from(handle.clone());
        drv.switch_to_window(hh)
            .await
            .map_err(|e| anyhow!("switch_window({handle}) failed: {e}"))
    })
}

pub fn switch_frame(session: Option<u64>, element_id: u64) -> Result<()> {
    let _drv = resolve_session(session)?;
    let elem = get_element(element_id)?;
    block_on(async move {
        elem.enter_frame()
            .await
            .map_err(|e| anyhow!("switch_frame failed: {e}"))
    })
}

pub fn switch_default_content(session: Option<u64>) -> Result<()> {
    let drv = resolve_session(session)?;
    block_on(async move {
        drv.enter_default_frame()
            .await
            .map_err(|e| anyhow!("switch_default_content failed: {e}"))
    })
}

pub fn switch_parent_frame(session: Option<u64>) -> Result<()> {
    let drv = resolve_session(session)?;
    block_on(async move {
        drv.enter_parent_frame()
            .await
            .map_err(|e| anyhow!("switch_parent_frame failed: {e}"))
    })
}

// ── cookies ─────────────────────────────────────────────────────────────

pub fn cookies(session: Option<u64>) -> Result<Value> {
    let drv = resolve_session(session)?;
    block_on(async move {
        let cs = drv
            .get_all_cookies()
            .await
            .map_err(|e| anyhow!("cookies failed: {e}"))?;
        serde_json::to_value(&cs).map_err(|e| anyhow!("cookies serialize: {e}"))
    })
}

pub fn add_cookie(session: Option<u64>, opts: Value) -> Result<()> {
    let drv = resolve_session(session)?;
    let cookie: thirtyfour::Cookie =
        serde_json::from_value(opts).map_err(|e| anyhow!("add_cookie: invalid opts: {e}"))?;
    block_on(async move {
        drv.add_cookie(cookie)
            .await
            .map_err(|e| anyhow!("add_cookie failed: {e}"))
    })
}

pub fn delete_cookie(session: Option<u64>, name: String) -> Result<()> {
    let drv = resolve_session(session)?;
    block_on(async move {
        drv.delete_cookie(&name)
            .await
            .map_err(|e| anyhow!("delete_cookie({name}) failed: {e}"))
    })
}

pub fn delete_all_cookies(session: Option<u64>) -> Result<()> {
    let drv = resolve_session(session)?;
    block_on(async move {
        drv.delete_all_cookies()
            .await
            .map_err(|e| anyhow!("delete_all_cookies failed: {e}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_rect_serializes_with_xywh_keys() {
        let r = WindowRect {
            x: -10,
            y: 20,
            width: 1280,
            height: 800,
        };
        let v: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert_eq!(v["x"], -10);
        assert_eq!(v["y"], 20);
        assert_eq!(v["width"], 1280);
        assert_eq!(v["height"], 800);
        assert_eq!(v.as_object().unwrap().len(), 4);
    }
}
