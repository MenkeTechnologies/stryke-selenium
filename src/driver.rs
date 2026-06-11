//! Browser launch / teardown + navigation + locator parsing.

use anyhow::{anyhow, Result};
use thirtyfour::prelude::*;
use thirtyfour::ChromiumLikeCapabilities;

use crate::common::{
    block_on, drain_sessions, register_session, resolve_session, take_session,
    DEFAULT_WEBDRIVER_URL,
};

/// Open a new WebDriver session. `browser` selects the capabilities table
/// (chrome / firefox / safari / edge). `url` overrides the default
/// WebDriver server location. `headless` flips the relevant
/// `--headless`-style flag for chrome / firefox / edge (safari has no
/// headless mode).
pub fn open(browser: &str, url: Option<&str>, headless: bool) -> Result<u64> {
    let server = url.unwrap_or(DEFAULT_WEBDRIVER_URL).to_string();
    let browser_norm = browser.to_ascii_lowercase();
    block_on(async move {
        let driver = match browser_norm.as_str() {
            "chrome" => {
                let mut caps = DesiredCapabilities::chrome();
                if headless {
                    // `--headless=new` is the modern (chromium 109+) flag.
                    // Older `--headless` still works but routes through the
                    // legacy headless path; new is what Selenium docs
                    // recommend as of 2024+.
                    caps.add_arg("--headless=new")?;
                    caps.add_arg("--disable-gpu")?;
                }
                WebDriver::new(&server, caps).await
            }
            "firefox" => {
                let mut caps = DesiredCapabilities::firefox();
                if headless {
                    caps.set_headless()?;
                }
                WebDriver::new(&server, caps).await
            }
            "safari" => {
                // Safari has no headless mode — `safaridriver` runs the
                // visible app. If the caller asked for headless, fail
                // loudly rather than silently lying about the mode.
                if headless {
                    return Err(anyhow!(
                        "safari has no headless mode — drop headless=1 or pick chrome/firefox"
                    ));
                }
                let caps = DesiredCapabilities::safari();
                WebDriver::new(&server, caps).await
            }
            "edge" => {
                let mut caps = DesiredCapabilities::edge();
                if headless {
                    caps.add_arg("--headless=new")?;
                    caps.add_arg("--disable-gpu")?;
                }
                WebDriver::new(&server, caps).await
            }
            other => {
                return Err(anyhow!(
                    "unsupported browser '{other}' — pick one of: chrome, firefox, safari, edge"
                ))
            }
        }
        .map_err(|e| anyhow!("WebDriver::new({server}) failed: {e}"))?;
        register_session(driver)
    })
}

/// Close session `id` and drop it from the registry.
pub fn quit(id: u64) -> Result<()> {
    let drv = take_session(id)?;
    block_on(async move {
        drv.quit()
            .await
            .map_err(|e| anyhow!("WebDriver::quit failed: {e}"))
    })
}

/// Close every open session. Best-effort — a `quit()` failure on one
/// session doesn't abort the others (we still drained them from the
/// registry; the stryke process is presumably exiting). Returns the count
/// of sessions whose `quit()` succeeded.
pub fn quit_all() -> Result<usize> {
    let drained = drain_sessions()?;
    block_on(async move {
        let mut ok = 0usize;
        for (_id, drv) in drained {
            if drv.quit().await.is_ok() {
                ok += 1;
            }
        }
        Ok(ok)
    })
}

pub fn goto(id: Option<u64>, url: &str) -> Result<()> {
    let drv = resolve_session(id)?;
    let url = url.to_string();
    block_on(async move {
        drv.goto(&url)
            .await
            .map_err(|e| anyhow!("goto({url}) failed: {e}"))
    })
}

pub fn current_url(id: Option<u64>) -> Result<String> {
    let drv = resolve_session(id)?;
    block_on(async move {
        let u = drv
            .current_url()
            .await
            .map_err(|e| anyhow!("current_url failed: {e}"))?;
        Ok(u.to_string())
    })
}

pub fn title(id: Option<u64>) -> Result<String> {
    let drv = resolve_session(id)?;
    block_on(async move { drv.title().await.map_err(|e| anyhow!("title failed: {e}")) })
}

pub fn source(id: Option<u64>) -> Result<String> {
    let drv = resolve_session(id)?;
    block_on(async move {
        drv.source()
            .await
            .map_err(|e| anyhow!("source failed: {e}"))
    })
}

pub fn back(id: Option<u64>) -> Result<()> {
    let drv = resolve_session(id)?;
    block_on(async move { drv.back().await.map_err(|e| anyhow!("back failed: {e}")) })
}

pub fn forward(id: Option<u64>) -> Result<()> {
    let drv = resolve_session(id)?;
    block_on(async move {
        drv.forward()
            .await
            .map_err(|e| anyhow!("forward failed: {e}"))
    })
}

pub fn refresh(id: Option<u64>) -> Result<()> {
    let drv = resolve_session(id)?;
    block_on(async move {
        drv.refresh()
            .await
            .map_err(|e| anyhow!("refresh failed: {e}"))
    })
}

pub fn set_implicit_wait(id: Option<u64>, seconds: f64) -> Result<()> {
    let drv = resolve_session(id)?;
    let dur = std::time::Duration::from_secs_f64(seconds.max(0.0));
    block_on(async move {
        drv.set_implicit_wait_timeout(dur)
            .await
            .map_err(|e| anyhow!("set_implicit_wait failed: {e}"))
    })
}

pub fn set_page_load_timeout(id: Option<u64>, seconds: f64) -> Result<()> {
    let drv = resolve_session(id)?;
    let dur = std::time::Duration::from_secs_f64(seconds.max(0.0));
    block_on(async move {
        drv.set_page_load_timeout(dur)
            .await
            .map_err(|e| anyhow!("set_page_load_timeout failed: {e}"))
    })
}

pub fn set_script_timeout(id: Option<u64>, seconds: f64) -> Result<()> {
    let drv = resolve_session(id)?;
    let dur = std::time::Duration::from_secs_f64(seconds.max(0.0));
    block_on(async move {
        drv.set_script_timeout(dur)
            .await
            .map_err(|e| anyhow!("set_script_timeout failed: {e}"))
    })
}

/// Map a stryke-side locator strategy name to a thirtyfour `By` variant.
/// Built inline at the call site so the selector string's lifetime is
/// bounded by the surrounding async block — no per-call `Box::leak`.
///
/// `selector` is taken by value because thirtyfour's `By` variants own
/// their string (the exact owned type — `String`, `Cow<'static, str>`,
/// etc. — has drifted across thirtyfour versions; `.into()` adapts to
/// whichever shape this build of the crate exposes).
pub fn by_from(strategy: &str, selector: String) -> Result<By> {
    let s = selector;
    match strategy.to_ascii_lowercase().as_str() {
        "css" | "css_selector" => Ok(By::Css(s)),
        "id" => Ok(By::Id(s)),
        "name" => Ok(By::Name(s)),
        "xpath" => Ok(By::XPath(s)),
        "tag" | "tag_name" => Ok(By::Tag(s)),
        "class" | "class_name" => Ok(By::ClassName(s)),
        "link_text" | "link" => Ok(By::LinkText(s)),
        "partial_link_text" | "plink" => Ok(By::PartialLinkText(s)),
        other => Err(anyhow!(
            "unknown locator strategy '{other}' — pick one of: css, id, name, xpath, tag, class, link_text, partial_link_text"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // thirtyfour's `By` keeps its inner `BySelector` private, but `By: Display`
    // (delegating to `BySelector::Display`) is the only public surface that
    // distinguishes which variant was chosen AND what query string it carries.
    // That's exactly what we want to pin: a refactor of the `by_from` match
    // that swaps the variant or mangles the selector would change this string.
    fn rendered(strategy: &str, selector: &str) -> String {
        format!("{}", by_from(strategy, selector.to_string()).unwrap())
    }

    #[test]
    fn strategy_match_is_ascii_case_insensitive() {
        // `by_from` lower-cases the strategy before matching. A caller passing
        // "XPath" / "CSS" (the casing selenium docs use) must NOT fall into the
        // unknown-strategy error arm. Pin both the case-fold AND the variant so
        // a regression that drops `.to_ascii_lowercase()` is caught here rather
        // than as a runtime "unknown locator strategy" at the WebDriver call.
        assert_eq!(rendered("XPath", "//a[@id='x']"), "XPath(//a[@id='x'])");
        assert_eq!(rendered("CSS", "div.box"), "CSS(div.box)");
        assert_eq!(rendered("ID", "main"), "Id(main)");
    }

    #[test]
    fn aliases_resolve_to_the_same_selector() {
        // Each canonical strategy has a documented alias that must produce a
        // byte-identical selector. If the alias arm drifts (e.g. someone maps
        // "css_selector" to By::Id by mistake, or forgets the `| "link"` arm),
        // these equalities break. The `name`/`tag`/`class` cases also pin the
        // CSS-rewrite thirtyfour applies (Name -> `[name="q"]`, class -> `.q`),
        // so a stryke script using `by => "name"` keeps targeting the attribute
        // selector and not, say, a literal tag named "q".
        assert_eq!(rendered("css_selector", "a"), rendered("css", "a"));
        assert_eq!(rendered("tag_name", "div"), rendered("tag", "div"));
        assert_eq!(rendered("class_name", "btn"), rendered("class", "btn"));
        assert_eq!(rendered("link", "Home"), rendered("link_text", "Home"));
        assert_eq!(rendered("plink", "Ho"), rendered("partial_link_text", "Ho"));
        // Name is rewritten to a CSS attribute selector, not a bare token.
        assert_eq!(rendered("name", "q"), r#"CSS([name="q"])"#);
        assert_eq!(rendered("tag", "q"), "CSS(q)");
    }

    #[test]
    fn unknown_strategy_errors_and_quotes_the_offender() {
        // Empty string and a near-miss ("xpaths") must error rather than
        // silently defaulting to CSS — a silent default would send a malformed
        // selector to WebDriver and surface as a confusing find() failure far
        // from the typo. The error text must echo the bad strategy so the
        // stryke `die` message is actionable.
        let err = by_from("xpaths", "//a".to_string())
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("xpaths"),
            "error should name the bad strategy: {err}"
        );
        assert!(by_from("", "x".to_string()).is_err());
    }

    #[test]
    fn selector_with_unicode_passes_through_untouched() {
        // The strategy is case-folded but the SELECTOR must not be — it is
        // taken by value and handed to `By::*` verbatim. A multi-byte selector
        // (emoji + combining chars) would corrupt if anyone added byte-index
        // slicing or case-folding to the selector path. Pin the round-trip.
        let sel = "a[title=\"café 🚀\"]";
        assert_eq!(rendered("css", sel), format!("CSS({sel})"));
    }
}
