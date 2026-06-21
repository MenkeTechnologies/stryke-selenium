//! Action-chain operations — low-level pointer/keyboard sequences the plain
//! `element.click()` / `element.send_keys()` calls can't express (right-click,
//! double-click, hover-then-act, key chords against the focused element).
//!
//! Each op builds a fresh `ActionChain` off the session, queues the moves, and
//! `perform()`s it in one round-trip. Element ids are resolved to live
//! `WebElement` handles before the chain is built so a stale id errors with the
//! original id intact rather than as an opaque WebDriver fault.

use anyhow::{anyhow, Result};

use crate::common::{block_on, get_element, resolve_session};

/// Move the pointer to `element_id`'s center and left-click it. Mirrors
/// `ActionChain::click_element` — useful when a plain `element.click()` is
/// intercepted (overlay, custom pointer handlers) and a real pointer move is
/// required first.
pub fn action_click(session: Option<u64>, element_id: u64) -> Result<()> {
    let drv = resolve_session(session)?;
    let elem = get_element(element_id)?;
    block_on(async move {
        drv.action_chain()
            .click_element(&elem)
            .perform()
            .await
            .map_err(|e| anyhow!("action_click failed: {e}"))
    })
}

/// Move the pointer to `element_id`'s center and double-click it.
pub fn action_double_click(session: Option<u64>, element_id: u64) -> Result<()> {
    let drv = resolve_session(session)?;
    let elem = get_element(element_id)?;
    block_on(async move {
        drv.action_chain()
            .double_click_element(&elem)
            .perform()
            .await
            .map_err(|e| anyhow!("action_double_click failed: {e}"))
    })
}

/// Move the pointer to `element_id`'s center and right-click (context-click) it.
pub fn action_context_click(session: Option<u64>, element_id: u64) -> Result<()> {
    let drv = resolve_session(session)?;
    let elem = get_element(element_id)?;
    block_on(async move {
        drv.action_chain()
            .context_click_element(&elem)
            .perform()
            .await
            .map_err(|e| anyhow!("action_context_click failed: {e}"))
    })
}

/// Type `text` against whatever element currently has focus, via a key-down /
/// key-up action sequence (not the element-scoped `send_keys`). Combine with
/// `Selenium::key_code` chars to send chords / control keys.
pub fn action_send_keys(session: Option<u64>, text: String) -> Result<()> {
    let drv = resolve_session(session)?;
    block_on(async move {
        drv.action_chain()
            .send_keys(text)
            .perform()
            .await
            .map_err(|e| anyhow!("action_send_keys failed: {e}"))
    })
}
