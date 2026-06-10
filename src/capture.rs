//! Screenshots — full-page and per-element. thirtyfour's
//! `screenshot_as_png()` returns the encoded PNG bytes directly; we hand
//! those back to stryke either as a written file (when the caller passes
//! `output`) or as a `\@bytes` array embedded in the JSON return.

use std::fs;

use anyhow::{anyhow, Result};
use serde::Serialize;

use crate::common::{block_on, get_element, resolve_session};

#[derive(Serialize)]
pub struct ScreenshotRaw {
    pub png: Vec<u8>,
}

pub fn screenshot(session: Option<u64>, path: Option<&str>) -> Result<ScreenshotRet> {
    let drv = resolve_session(session)?;
    let path_owned = path.map(String::from);
    block_on(async move {
        let png = drv
            .screenshot_as_png()
            .await
            .map_err(|e| anyhow!("screenshot failed: {e}"))?;
        finalize(png, path_owned)
    })
}

pub fn element_screenshot(id: u64, path: Option<&str>) -> Result<ScreenshotRet> {
    let elem = get_element(id)?;
    let path_owned = path.map(String::from);
    block_on(async move {
        let png = elem
            .screenshot_as_png()
            .await
            .map_err(|e| anyhow!("element_screenshot failed: {e}"))?;
        finalize(png, path_owned)
    })
}

pub enum ScreenshotRet {
    Path(String),
    Raw(ScreenshotRaw),
}

fn finalize(png: Vec<u8>, path: Option<String>) -> Result<ScreenshotRet> {
    match path {
        Some(p) => {
            fs::write(&p, &png).map_err(|e| anyhow!("write {p}: {e}"))?;
            Ok(ScreenshotRet::Path(p))
        }
        None => Ok(ScreenshotRet::Raw(ScreenshotRaw { png })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screenshot_raw_serializes_with_png_key() {
        let s = ScreenshotRaw {
            png: vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A],
        };
        let v: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(v["png"].as_array().unwrap().len(), 8);
        // PNG magic byte 0 is 0x89 = 137. If serde changes to base64
        // encoding under us, this guard catches it.
        assert_eq!(v["png"][0], 137);
    }
}
