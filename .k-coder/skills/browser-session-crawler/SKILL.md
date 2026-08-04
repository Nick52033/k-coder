---
name: browser-session-crawler
description: Crawl websites using your logged-in Chrome/Edge browser session. Automatically reuses existing login state; if not logged in, shows a popup to remind you to login, then continues automatically. Ideal for sites requiring authentication (social media, communities, admin panels, etc.).
triggers:
  - browser session crawler
  - 登录态爬取
  - 浏览器会话爬取
risk: external
enabled: true
---

# Browser Session Crawler

Crawl websites using your system's logged-in Chrome/Edge browser session.

Compatibility: Requires Python 3.8+. Dependencies: `playwright` (`pip install playwright && playwright install chromium`).

## Core Features

- **? Automatic Session Reuse** - Uses Chrome/Edge user data directory, no need to login again
- **⏳ Login Reminder** - Detects unauthenticated state, shows popup reminder, continues after login
- **? Real Browser Environment** - Non-headless mode, fewer anti-bot detections
- **? Pre-built Crawlers** - Ready-to-use scripts for Xiaohongshu (Redbook), Zhihu, and more

## Installation

```bash
pip install playwright
playwright install chromium
```

## Quick Start

### Xiaohongshu Crawler (Recommended)

```bash
# Search for beach beauty photos
python scripts/xiaohongshu.py "beach beauty" --count 20

# Search for any keyword
python scripts/xiaohongshu.py "your keyword"
```

**Parameters:**

| Parameter | Required | Description |
|-----------|----------|-------------|
| `keyword` | ✅ | Search keyword |
| `--count` | No | Number of items to crawl (default: 20) |
| `--save` | No | Directory to save images |

**Examples:**

```bash
# Crawl 50 beach beauty photos, save to imgs folder
python scripts/xiaohongshu.py "beach beauty" --count 50 --save imgs
```

### Generic Crawler

```bash
python scripts/crawl.py "target_URL" --logged-indicator "login_indicator" --selector "css_selector"
```

| Parameter | Required | Description |
|-----------|----------|-------------|
| `target_url` | ✅ | Target page URL |
| `--logged-indicator` | ✅ | CSS selector that appears only after login |
| `--selector` | No | CSS selector for elements to extract |
| `--wait` | No | Seconds to wait after page load (default: 3) |
| `--scroll` | No | Scroll page to trigger lazy loading |
| `--max-length` | No | Maximum character count for output |
| `--save` | No | Save output to file |

### iDataPlatform API Detail Capture

For IPSA iDataPlatform service search pages, prefer network response capture over visible text extraction.

Run from the workspace where captured temporary output should be written:

```bash
python .codex/skills/browser-session-crawler/scripts/capture-idata-api-detail.py "项目主数据" --login-timeout 900 --response-timeout 120 --action-timeout 30 --keep-open-on-error
```

In Codex terminal sessions on Windows, prefer launching the same script through
`runpy` so the visible browser starts reliably even when direct script-path
execution is rejected by the unified exec launcher:

```bash
python -X utf8 -c "import runpy, sys; sys.argv=[r'C:\Users\nealk\.codex\skills\browser-session-crawler\scripts\capture-idata-api-detail.py','项目主数据','--login-timeout','900','--response-timeout','120','--action-timeout','30','--keep-open-on-error']; runpy.run_path(sys.argv[0], run_name='__main__')"
```

Default wait policy for iDataPlatform capture: wait up to 900 seconds for manual login, 120 seconds for the detail network response, and 30 seconds for Playwright actions. If a user asks for a longer browser wait, increase these values instead of shortening them.

If the browser opens and closes immediately, rerun with `--keep-open-on-error` and inspect `.tmp-browser-session-crawler/output/error.json`, `error.png`, and `error.txt`.

Workflow:

1. Open `https://ipsapro.isoftstone.com/iDataPlatform/serviceHome/serviceSearch`.
2. If redirected to a login page, click the login method item `.item_oFCaWe7B` first, then let the user type account/password in the visible browser.
3. After login, search the requested keyword, for example `项目主数据`.
4. Before clicking the first result's `详情`, start waiting for a network response whose URL contains `getApiByDeatail`; also accept `getApiByDetail` as a spelling fallback.
5. Click `详情`.
6. Save the captured response body as the source detail response, and append the same raw evidence to `evidence-YYYYMMDD.log`. Only fall back to DOM text when this response cannot be captured.

For iDataPlatform specs, the `getApiByDeatail` response is the authoritative source for fields such as `apiPath`, `apiMethod`, `apiParams`, and `apiResults`.
After the downstream interface spec has been generated successfully, temporary JSON capture files may be deleted; keep the dated `evidence-YYYYMMDD.log` file as the retained source evidence.

Useful script variants:

| Script | Function | Example |
|--------|----------|---------|
| `scripts/capture-idata-api-detail.py` | Search iDataPlatform and save `getApiByDeatail` / `getApiByDetail` response | `python .codex/skills/browser-session-crawler/scripts/capture-idata-api-detail.py "项目主数据"` |
| `scripts/capture-idata-project-master.py` | Project-master wrapper that also writes `.tmp_idata_project_master_response.json` | `python .codex/skills/browser-session-crawler/scripts/capture-idata-project-master.py` |
| `scripts/kill-browser-session-runner.py` | Stop lingering capture runner/browser processes for this workspace profile | `python .codex/skills/browser-session-crawler/scripts/kill-browser-session-runner.py` |

## Pre-built Scripts

| Script | Function | Example |
|--------|----------|---------|
| `xiaohongshu.py` | Xiaohongshu search crawler | `python scripts/xiaohongshu.py "food"` |
| `crawl.py` | Generic webpage crawler | `python scripts/crawl.py "url" --logged-indicator "..."` |
| `example_zhihu.py` | Zhihu crawler example | - |

## Common Site Configurations

### Xiaohongshu (Redbook)

```bash
# Search page crawling (auto extracts images)
python scripts/xiaohongshu.py "beach beauty"

# Generic method
python scripts/crawl.py "https://www.xiaohongshu.com/search_result?keyword=beauty" --logged-indicator ".user-avatar" --selector ".note-item"
```

### Zhihu

```bash
python scripts/crawl.py "https://www.zhihu.com/topic/19550517/hot" --logged-indicator ".AppHeader-profile" --selector ".List-item" --scroll
```

### Weibo

```bash
python scripts/crawl.py "https://weibo.com/hot/search" --logged-indicator ".user-name" --selector ".list_pub" --scroll
```

## Login Detection

Uses `--logged-indicator` selector to detect login state:
- Element found → Logged in, proceed with crawling
- Timeout (not found) → Show login reminder → Continue after login

**Common Login Indicators:**

| Site | Selector |
|------|----------|
| Xiaohongshu | `.user-avatar`, `.profile-avatar`, `.user-name` |
| Zhihu | `.AppHeader-profile`, `.UserAvatar` |
| LinkedIn | `.global-nav__me-wrapper` |
| Weibo | `.user-name`, `.m-text-cut` |

## Workflow

```
1. Detect system browser user data directory
       ↓
2. Launch Chromium (reuse logged-in session)
       ↓
3. Navigate to target page
       ↓
4. Check login status
       ↓
   ┌─────────────┐
   │  Logged in? │
   └─────────────┘
      ↓       ↓
    Yes       No
      ↓       ↓
   Crawl  Show login reminder
      ↓       ↓
   Save results
```

## Troubleshooting

| Issue | Solution |
|-------|----------|
| Browser launch failed | Check if Chrome/Edge is currently using user data directory |
| Login detection failed | Adjust `--logged-indicator` to correct selector |
| Empty content | Increase `--wait 5` or add `--scroll` |
| Page stuck | Try `--headless` mode (may not support login) |
