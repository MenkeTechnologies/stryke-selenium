```
 ███████╗████████╗██████╗ ██╗   ██╗██╗  ██╗███████╗
 ██╔════╝╚══██╔══╝██╔══██╗╚██╗ ██╔╝██║ ██╔╝██╔════╝
 ███████╗   ██║   ██████╔╝ ╚████╔╝ █████╔╝ █████╗
 ╚════██║   ██║   ██╔══██╗  ╚██╔╝  ██╔═██╗ ██╔══╝
 ███████║   ██║   ██║  ██║   ██║   ██║  ██╗███████╗
 ╚══════╝   ╚═╝   ╚═╝  ╚═╝   ╚═╝   ╚═╝  ╚═╝╚══════╝
                [ s e l e n i u m ]
```

[![CI](https://github.com/MenkeTechnologies/stryke-selenium/actions/workflows/ci.yml/badge.svg)](https://github.com/MenkeTechnologies/stryke-selenium/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![stryke](https://img.shields.io/badge/stryke-package-cyan.svg)](https://github.com/MenkeTechnologies/strykelang)

### `[BROWSER AUTOMATION FOR STRYKE // WEBDRIVER + DOM + JS + COOKIES]`

> *"selenium-python, one stryke pipe away."*

Selenium WebDriver automation for stryke — browser launch (chrome /
firefox / safari / edge, headless or visible), navigation, element
queries (css / xpath / id / name / tag / class / link-text), click /
send_keys / clear, attribute / property / CSS reads, JavaScript
execution, screenshots (full-page + per-element), window + frame
control, and cookie management. Shipped as a precompiled cdylib that
stryke dlopens in-process on first `use Selenium`. A process-global
tokio runtime bridges thirtyfour's async API to the sync FFI; WebDriver
sessions and WebElement handles persist across calls.

### [`strykelang`](https://github.com/MenkeTechnologies/strykelang) &middot; [`stryke-gui`](https://github.com/MenkeTechnologies/stryke-gui) &middot; [`stryke-aws`](https://github.com/MenkeTechnologies/stryke-aws)

---

## Table of Contents

- [\[0x00\] How this loads](#0x00-how-this-loads)
- [\[0x01\] Install](#0x01-install)
- [\[0x02\] Quick start](#0x02-quick-start)
- [\[0x03\] API reference](#0x03-api-reference)
- [\[0x04\] Launching a WebDriver server](#0x04-launching-a-webdriver-server)
- [\[0x05\] Examples](#0x05-examples)
- [\[0x06\] Tests](#0x06-tests)
- [\[0x07\] Build from source](#0x07-build-from-source)
- [\[0x08\] Layout](#0x08-layout)
- [\[0xFF\] License](#0xff-license)

---

## [0x00] How this loads

`stryke-selenium` is a cdylib package: each `extern "C" fn selenium__*`
in `src/lib.rs` is a JSON-string-in / JSON-string-out wrapper around the
`driver` / `element` / `script` / `capture` / `window` modules. On first
`use Selenium`:

1. stryke's package resolver finds the installed package in
   `~/.stryke/store/selenium@<version>/`.
2. The package's `[ffi]` section names the exports.
3. stryke `dlopen`s `lib/libstryke_selenium.{dylib,so}` next to
   `lib/Selenium.stk`.
4. Every export gets registered in stryke's FFI registry with signature
   `*const c_char -> *const c_char`.
5. The `lib/Selenium.stk` wrapper just JSON-encodes args, calls the FFI
   symbol, and parses the JSON return.

Every `Selenium::*` call is a direct function call into the cdylib — no
`fork(2)`, no `exec(2)`, no JSON-over-pipe round-trip, no
`WebDriver::new()` per invocation. The async bridge is one process-wide
`tokio::runtime::Runtime` (built on first FFI entry, see
`src/common.rs::runtime`), and the WebDriver session lives in a
`OnceCell<Mutex<HashMap<u64, WebDriver>>>` registry. WebElement handles
get the same treatment — a client-side `u64` keys a registry of live
`WebElement`s so the cdylib never re-finds an element you already have
a handle to.

Multiple browser sessions are first-class: every `Selenium::open()`
returns a fresh integer session id. Calls default to the "active"
session (set on first open, changeable via `Selenium::set_active`); pass
an explicit `$sid` as the last argument to address a specific browser.

## [0x01] Install

Stryke must be installed first (see [strykelang](https://github.com/MenkeTechnologies/strykelang)).
Then, on macOS or Linux:

```sh
s pkg install -g github.com/MenkeTechnologies/stryke-selenium
```

This fetches the prebuilt release tarball for your host triple
(`aarch64-apple-darwin`, `x86_64-apple-darwin`,
`x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`), verifies its
SHA-256, extracts into `~/.stryke/store/selenium@<version>/`, and
registers the cdylib for `use Selenium`. No `cargo`, no `rustc`, no
per-target build step on the user's machine.

Pin a specific release:

```sh
s pkg install -g github.com/MenkeTechnologies/stryke-selenium@v0.1.0
```

You also need a WebDriver server (chromedriver / geckodriver /
safaridriver) running — see [\[0x04\]](#0x04-launching-a-webdriver-server).

## [0x02] Quick start

```perl
use Selenium

# launch a browser
Selenium::open(browser => "chrome", headless => 1)

# navigate + read
Selenium::goto("https://example.com")
p "title: " . Selenium::title()
p "url:   " . Selenium::current_url()

# find + interact
val $h1 = Selenium::find("h1")
p "h1 text: " . Selenium::text($h1)

# wait + click
val $btn = Selenium::wait_for("button.submit", "css", 10)
Selenium::click($btn)

# screenshot
Selenium::screenshot("/tmp/page.png")

# clean up
Selenium::quit()
```

## [0x03] API reference

All functions live in the `Selenium::` namespace (`use Selenium`). The
last optional argument on most calls is `$sid` — the session id returned
by `Selenium::open`. Omit it to use the active session.

### Session lifecycle

| Function | Notes |
|----------|-------|
| `Selenium::open(%opts)` | `browser` (chrome/firefox/safari/edge, default chrome), `url` (WebDriver server URL, default `http://localhost:9515`), `headless` (1/0, default 0). Returns the session id. |
| `Selenium::quit($sid?)` | Close one session. |
| `Selenium::quit_all()` | Close every open session. Returns count closed. |
| `Selenium::sessions()` | List of open session ids. |
| `Selenium::active()` | Active session id, or `undef`. |
| `Selenium::set_active($sid)` | Set the active session. |
| `Selenium::supported_browsers()` | `("chrome", "firefox", "safari", "edge")` |
| `Selenium::locator_strategies()` | Every name accepted as `$by`. |

### Navigation

| Function | Notes |
|----------|-------|
| `Selenium::goto($url, $sid?)` / `Selenium::get(...)` | Navigate. |
| `Selenium::current_url($sid?)` | |
| `Selenium::title($sid?)` | |
| `Selenium::source($sid?)` | Full HTML. |
| `Selenium::back($sid?)` / `forward` / `refresh` | |
| `Selenium::set_implicit_wait($s, $sid?)` | Server-side implicit-wait timeout. |
| `Selenium::set_page_load_timeout($s, $sid?)` | |
| `Selenium::set_script_timeout($s, $sid?)` | |

### Element queries

| Function | Notes |
|----------|-------|
| `Selenium::find($sel, $by="css", $sid?)` | Returns one element id, or dies. |
| `Selenium::find_all($sel, $by="css", $sid?)` | Returns a list of ids. |
| `Selenium::wait_for($sel, $by="css", $timeout=10, $sid?)` | Polls every 200 ms until found or timeout. |

`$by` values: `css` (default), `id`, `name`, `xpath`, `tag`, `class`,
`link_text`, `partial_link_text`. Each has short aliases — see
`Selenium::locator_strategies()`.

### Element ops

| Function | Returns |
|----------|---------|
| `Selenium::click($eid)` | |
| `Selenium::send_keys($eid, $text)` | |
| `Selenium::clear($eid)` | |
| `Selenium::text($eid)` | visible text |
| `Selenium::attr($eid, $name)` | HTML attribute or `undef` |
| `Selenium::prop($eid, $name)` | live DOM property |
| `Selenium::css($eid, $name)` | resolved CSS value |
| `Selenium::tag($eid)` | lowercase tag name |
| `Selenium::rect($eid)` | `($x, $y, $w, $h)` |
| `Selenium::is_displayed($eid)` / `is_enabled` / `is_selected` | `1` / `0` |
| `Selenium::drop($eid)` | drop the client-side handle |

### JavaScript

| Function | Notes |
|----------|-------|
| `Selenium::execute_script($js, $args?, $sid?)` | Runs `$js` with `$args` as `arguments[0..N]`. Returns whatever `return ...` produced, parsed from JSON. To pass a Selenium element, wrap: `{ __element__ => $eid }`. |

### Screenshots

| Function | Returns |
|----------|---------|
| `Selenium::screenshot($path?, $sid?)` | `$path` if given, else `\@png_bytes` |
| `Selenium::element_screenshot($eid, $path?)` | same shape |

### Window / frame

| Function | Notes |
|----------|-------|
| `Selenium::window_rect($sid?)` | `($x, $y, $w, $h)` |
| `Selenium::set_window_rect($x?, $y?, $w?, $h?, $sid?)` | partial accepted |
| `Selenium::set_window_size($w, $h, $sid?)` | keeps position |
| `Selenium::set_window_position($x, $y, $sid?)` | keeps size |
| `Selenium::window_handles($sid?)` | every tab/window handle |
| `Selenium::current_window($sid?)` | handle of focused tab |
| `Selenium::switch_window($handle, $sid?)` | |
| `Selenium::switch_frame($eid, $sid?)` | enter iframe by element |
| `Selenium::switch_default_content($sid?)` | back to top |
| `Selenium::switch_parent_frame($sid?)` | one level out |

### Cookies

| Function | Notes |
|----------|-------|
| `Selenium::cookies($sid?)` | arrayref of cookie hashes |
| `Selenium::add_cookie(%fields)` | name + value required; path, domain, secure, http_only, same_site, expiry optional |
| `Selenium::delete_cookie($name, $sid?)` | |
| `Selenium::delete_all_cookies($sid?)` | |

## [0x04] Launching a WebDriver server

`stryke-selenium` is a client. You launch the WebDriver server yourself.
One-liners per browser:

```sh
# Chrome — default. Matches Selenium::open() with no url arg.
brew install --cask chromedriver
chromedriver --port=9515 &

# Firefox — pass url => "http://localhost:4444" to Selenium::open.
brew install geckodriver
geckodriver --port 4444 &

# Safari — macOS only; one-time enable:
safaridriver --enable
safaridriver -p 4444 &
# Selenium::open(browser => "safari", url => "http://localhost:4444")

# Edge — install msedgedriver matching your Edge version.
msedgedriver --port=9515 &
# Selenium::open(browser => "edge")

# Selenium Grid 4 (all browsers via one server):
brew install selenium-server
selenium-server standalone --port 4444 &
# Selenium::open(browser => "chrome", url => "http://localhost:4444")
```

To verify the install + roundtrip without launching a browser:

```sh
selenium-test
```

(installed at `~/.stryke/bin/selenium-test` by `s pkg install -g`).

## [0x05] Examples

```sh
s examples/selenium_basic.stk          # open chrome, get title, quit
s examples/selenium_headless.stk       # same, no visible window
s examples/selenium_form_fill.stk      # find input, send_keys, submit
s examples/selenium_screenshot.stk     # full-page + per-element capture
s examples/selenium_wait.stk           # wait_for with 10s timeout
s examples/selenium_cookies.stk        # add / list / delete cookies
s examples/selenium_js.stk             # execute_script with WebElement args
s examples/selenium_windows.stk        # multi-tab switch
s examples/selenium_multi_session.stk  # two browsers in parallel
```

## [0x06] Tests

```sh
make test            # cargo test + `s test t/`
```

`cargo test` covers the FFI plumbing (JSON-in/out wrapper, error-on-panic
behavior, free-cstring contract, session/element registries, static
dispatch). `t/test_selenium.stk` covers the end-to-end stryke → FFI →
cdylib call path via the permission-free
`Selenium::supported_browsers()` / `locator_strategies()` /
`sessions()` queries. Live browser ops can't run unattended in CI
without a WebDriver server, so they're exercised by the
`examples/selenium_*.stk` demos against a locally-launched chromedriver.

## [0x07] Build from source

Consumers don't need this — the install path fetches a prebuilt
artifact for the host triple from GitHub Releases. Contributors building
the cdylib locally:

```sh
cd ~/RustroverProjects/stryke-selenium
cargo build --release            # → target/release/libstryke_selenium.{dylib,so}
```

thirtyfour ships with rustls-tls in the default feature set, so there
are no OS-side build dependencies beyond a working Rust toolchain — no
openssl, no Wayland / X11 stack.

Stryke's FFI loader looks for the cdylib in `lib/`, then
`target/release/`, then `target/debug/` (see `try_load_ffi_for` in
`strykelang/strykelang/pkg/commands.rs`). So once `cargo build` produces
the dev artifact, `s examples/selenium_basic.stk` works against the
local checkout without a separate install step.

To install the local build into the global store as a drop-in for a
released version:

```sh
s pkg install -g .
```

## [0x08] Layout

```
stryke.toml             stryke package manifest with [ffi] table
Cargo.toml              stryke_selenium cdylib crate manifest
src/
  lib.rs                #[no_mangle] extern "C" selenium__* exports + ffi_call wrapper
  common.rs             tokio runtime + session + element registries
  driver.rs             open / quit / navigation / locator parsing
  element.rs            find / wait_for / click / text / attr / ...
  script.rs             execute_script with WebElement arg unmarshaling
  capture.rs            page + per-element screenshots
  window.rs             window rects / handles / frames / cookies
lib/Selenium.stk        stryke wrappers (JSON args → FFI symbol → JSON return)
examples/               runnable demos
t/test_selenium.stk     plumbing tests (permission-free FFI surface)
bin/selenium-test.stk   installable smoke-test launcher
.github/workflows/
  ci.yml                cargo check/clippy/test/doc per push
  release.yml           per-triple cdylib build matrix → GitHub Release
```

## [0xFF] License

MIT © MenkeTechnologies
