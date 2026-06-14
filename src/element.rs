//! Element queries (find/find_all/wait_for) and element-side operations
//! (click, send_keys, text, attr, etc.).

use std::time::{Duration, Instant};

use crate::common::{block_on, drop_element, get_element, register_element, resolve_session};
use crate::driver::by_from;
use anyhow::{anyhow, Result};
use serde::Serialize;

#[derive(Serialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

pub fn find(session: Option<u64>, strategy: &str, selector: String) -> Result<u64> {
    let drv = resolve_session(session)?;
    let by = by_from(strategy, selector)?;
    block_on(async move {
        let elem = drv
            .find(by)
            .await
            .map_err(|e| anyhow!("find failed: {e}"))?;
        register_element(elem)
    })
}

pub fn find_all(session: Option<u64>, strategy: &str, selector: String) -> Result<Vec<u64>> {
    let drv = resolve_session(session)?;
    let by = by_from(strategy, selector)?;
    block_on(async move {
        let elems = drv
            .find_all(by)
            .await
            .map_err(|e| anyhow!("find_all failed: {e}"))?;
        let mut ids = Vec::with_capacity(elems.len());
        for e in elems {
            ids.push(register_element(e)?);
        }
        Ok(ids)
    })
}

/// Poll `find` until the element is present, or `timeout_s` elapses.
/// Polling interval is fixed at 200 ms — same default as selenium-python's
/// `WebDriverWait`. Returns the registered element id on success.
pub fn wait_for(
    session: Option<u64>,
    strategy: &str,
    selector: String,
    timeout_s: f64,
) -> Result<u64> {
    let drv = resolve_session(session)?;
    let strategy = strategy.to_string();
    let deadline = Instant::now() + Duration::from_secs_f64(timeout_s.max(0.0));
    let poll = Duration::from_millis(200);
    block_on(async move {
        let mut last_err: String;
        loop {
            let by = by_from(&strategy, selector.clone())?;
            match drv.find(by).await {
                Ok(elem) => return register_element(elem),
                Err(e) => last_err = e.to_string(),
            }
            if Instant::now() >= deadline {
                return Err(anyhow!("wait_for timed out after {timeout_s}s: {last_err}"));
            }
            tokio::time::sleep(poll).await;
        }
    })
}

pub fn click(id: u64) -> Result<()> {
    let elem = get_element(id)?;
    block_on(async move { elem.click().await.map_err(|e| anyhow!("click failed: {e}")) })
}

pub fn send_keys(id: u64, text: String) -> Result<()> {
    let elem = get_element(id)?;
    block_on(async move {
        elem.send_keys(&text)
            .await
            .map_err(|e| anyhow!("send_keys failed: {e}"))
    })
}

pub fn clear(id: u64) -> Result<()> {
    let elem = get_element(id)?;
    block_on(async move { elem.clear().await.map_err(|e| anyhow!("clear failed: {e}")) })
}

pub fn text(id: u64) -> Result<String> {
    let elem = get_element(id)?;
    block_on(async move { elem.text().await.map_err(|e| anyhow!("text failed: {e}")) })
}

pub fn scroll_into_view(id: u64) -> Result<()> {
    let elem = get_element(id)?;
    block_on(async move {
        elem.scroll_into_view()
            .await
            .map_err(|e| anyhow!("scroll_into_view failed: {e}"))
    })
}

pub fn attr(id: u64, name: String) -> Result<Option<String>> {
    let elem = get_element(id)?;
    block_on(async move {
        elem.attr(&name)
            .await
            .map_err(|e| anyhow!("attr({name}) failed: {e}"))
    })
}

pub fn prop(id: u64, name: String) -> Result<Option<String>> {
    let elem = get_element(id)?;
    block_on(async move {
        elem.prop(&name)
            .await
            .map_err(|e| anyhow!("prop({name}) failed: {e}"))
    })
}

pub fn css(id: u64, name: String) -> Result<String> {
    let elem = get_element(id)?;
    block_on(async move {
        elem.css_value(&name)
            .await
            .map_err(|e| anyhow!("css({name}) failed: {e}"))
    })
}

pub fn tag(id: u64) -> Result<String> {
    let elem = get_element(id)?;
    block_on(async move {
        elem.tag_name()
            .await
            .map_err(|e| anyhow!("tag failed: {e}"))
    })
}

pub fn rect(id: u64) -> Result<Rect> {
    let elem = get_element(id)?;
    block_on(async move {
        let r = elem.rect().await.map_err(|e| anyhow!("rect failed: {e}"))?;
        Ok(Rect {
            x: r.x,
            y: r.y,
            width: r.width,
            height: r.height,
        })
    })
}

pub fn is_displayed(id: u64) -> Result<bool> {
    let elem = get_element(id)?;
    block_on(async move {
        elem.is_displayed()
            .await
            .map_err(|e| anyhow!("is_displayed failed: {e}"))
    })
}

pub fn is_enabled(id: u64) -> Result<bool> {
    let elem = get_element(id)?;
    block_on(async move {
        elem.is_enabled()
            .await
            .map_err(|e| anyhow!("is_enabled failed: {e}"))
    })
}

pub fn is_selected(id: u64) -> Result<bool> {
    let elem = get_element(id)?;
    block_on(async move {
        elem.is_selected()
            .await
            .map_err(|e| anyhow!("is_selected failed: {e}"))
    })
}

/// Drop the element from the client-side registry. The server-side
/// WebDriver element id stays valid until the page is reloaded or the
/// session ends — calling `Selenium::find` again returns a fresh handle.
pub fn drop_id(id: u64) -> Result<bool> {
    drop_element(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_serializes_with_xywh_keys() {
        let r = Rect {
            x: 10.5,
            y: 20.0,
            width: 100.0,
            height: 50.25,
        };
        let v: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert_eq!(v["x"], 10.5);
        assert_eq!(v["y"], 20.0);
        assert_eq!(v["width"], 100.0);
        assert_eq!(v["height"], 50.25);
        assert_eq!(v.as_object().unwrap().len(), 4);
    }
}
