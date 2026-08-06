import argparse
import asyncio
import json
import os
import sys
import traceback
from datetime import datetime
from pathlib import Path

from playwright.async_api import TimeoutError as PlaywrightTimeoutError
from playwright.async_api import async_playwright


SERVICE_SEARCH_URL = "https://ipsapro.isoftstone.com/iDataPlatform/serviceHome/serviceSearch"
LOGIN_URL_MARKER = "ipsapro.isoftstone.com/portal"
DEFAULT_KEYWORD = "项目主数据"
DETAIL_API_KEYWORDS = ("getApiByDeatail", "getApiByDetail")


def ensure_dir(path: Path) -> Path:
    path.mkdir(parents=True, exist_ok=True)
    return path


async def save_page_artifacts(page, output_dir: Path, prefix: str) -> None:
    try:
        await page.screenshot(path=str(output_dir / f"{prefix}.png"), full_page=True)
    except Exception:
        pass

    try:
        text = await page.locator("body").inner_text(timeout=3000)
        (output_dir / f"{prefix}.txt").write_text(text, encoding="utf-8")
    except Exception:
        pass

    state = {
        "url": page.url,
        "title": await page.title(),
    }
    (output_dir / f"{prefix}.json").write_text(
        json.dumps(state, ensure_ascii=False, indent=2),
        encoding="utf-8",
    )


async def save_error_artifacts(page, output_dir: Path, exc: Exception) -> None:
    await save_page_artifacts(page, output_dir, "error")
    error_state = {
        "url": page.url,
        "title": await page.title(),
        "errorType": type(exc).__name__,
        "errorMessage": str(exc),
        "traceback": traceback.format_exc(),
    }
    (output_dir / "error.json").write_text(
        json.dumps(error_state, ensure_ascii=False, indent=2),
        encoding="utf-8",
    )


async def click_login_method_if_present(page) -> bool:
    selectors = [
        ".item_oFCaWe7B:has(img[src*='feilian'])",
        ".item_oFCaWe7B",
    ]
    for selector in selectors:
        locator = page.locator(selector)
        try:
            count = await locator.count()
        except Exception:
            continue
        for idx in range(min(count, 10)):
            item = locator.nth(idx)
            try:
                if await item.is_visible(timeout=1000):
                    await item.click(timeout=5000)
                    print(f"CLICK_LOGIN_METHOD {selector} index={idx}")
                    await page.wait_for_timeout(2000)
                    return True
            except Exception:
                continue
    return False


async def wait_for_login(page, output_dir: Path, timeout_seconds: int) -> None:
    await click_login_method_if_present(page)
    deadline = asyncio.get_running_loop().time() + timeout_seconds
    clicked_login_method = False

    while asyncio.get_running_loop().time() < deadline:
        current_url = page.url
        body_text = ""
        try:
            body_text = await page.locator("body").inner_text(timeout=1500)
        except Exception:
            pass

        if LOGIN_URL_MARKER in current_url and not clicked_login_method:
            clicked_login_method = await click_login_method_if_present(page)

        logged_in_markers = [
            "退出登录",
            "数据中台",
            "API首页",
            "API管理",
        ]
        if LOGIN_URL_MARKER not in current_url and any(marker in body_text for marker in logged_in_markers):
            return

        if LOGIN_URL_MARKER not in current_url and "serviceHome/serviceSearch" in current_url:
            return

        (output_dir / "login-waiting.json").write_text(
            json.dumps(
                {
                    "url": current_url,
                    "message": "Browser is open and waiting for interactive login.",
                },
                ensure_ascii=False,
                indent=2,
            ),
            encoding="utf-8",
        )
        await asyncio.sleep(2)

    raise TimeoutError(f"Login was not detected within {timeout_seconds} seconds.")


async def fill_search_box(page, keyword: str) -> None:
    selectors = [
        "input[placeholder*='API']",
        "input[placeholder*='名称']",
        "input[placeholder*='搜索']",
        ".ant-input",
        "input",
    ]
    for selector in selectors:
        locator = page.locator(selector).first
        try:
            if await locator.count() > 0 and await locator.is_visible(timeout=1500):
                await locator.fill(keyword)
                await locator.press("Enter")
                return
        except Exception:
            continue
    raise RuntimeError("Could not find a visible search input.")


async def click_search_button_if_present(page) -> None:
    selectors = [
        "button:has-text('搜索')",
        "text=搜索",
        ".ant-btn-primary:has-text('搜索')",
    ]
    for selector in selectors:
        locator = page.locator(selector).first
        try:
            if await locator.count() > 0 and await locator.is_visible(timeout=1000):
                await locator.click(timeout=3000)
                return
        except Exception:
            continue


async def wait_for_search_result(page, keyword: str) -> None:
    try:
        await page.locator(f"text={keyword}").first.wait_for(timeout=15000)
    except PlaywrightTimeoutError:
        await page.wait_for_timeout(3000)


async def click_first_detail(page) -> None:
    detail = page.locator("text=详情").first
    await detail.wait_for(timeout=20000)
    await detail.click(timeout=5000)


def is_detail_api_response(response) -> bool:
    return any(keyword in response.url for keyword in DETAIL_API_KEYWORDS)


def get_evidence_log_path(output_dir: Path) -> Path:
    return output_dir / f"evidence-{datetime.now().strftime('%Y%m%d')}.log"


async def save_detail_api_response(response, output_dir: Path):
    raw_text = await response.text()
    raw_path = output_dir / "getApiByDeatail.raw.json"
    json_path = output_dir / "getApiByDeatail.json"
    meta_path = output_dir / "getApiByDeatail.meta.json"
    evidence_path = get_evidence_log_path(output_dir)

    raw_path.write_text(raw_text, encoding="utf-8")
    parsed = None
    try:
        parsed = json.loads(raw_text)
        json_path.write_text(
            json.dumps(parsed, ensure_ascii=False, indent=2),
            encoding="utf-8",
        )
    except Exception:
        pass

    meta = {
        "url": response.url,
        "status": response.status,
        "contentType": response.headers.get("content-type", ""),
        "savedRaw": str(raw_path.resolve()),
        "savedJson": str(json_path.resolve()) if parsed is not None else None,
        "evidenceLog": str(evidence_path.resolve()),
    }
    meta_path.write_text(
        json.dumps(meta, ensure_ascii=False, indent=2),
        encoding="utf-8",
    )
    with evidence_path.open("a", encoding="utf-8") as evidence:
        evidence.write(f"\n=== iDataPlatform detail API response {datetime.now().isoformat(timespec='seconds')} ===\n")
        evidence.write(json.dumps(meta, ensure_ascii=False, indent=2))
        evidence.write("\n--- raw response ---\n")
        evidence.write(raw_text)
        evidence.write("\n=== end response ===\n")
    return meta


async def capture_detail_response(page, output_dir: Path, timeout_seconds: int):
    async with page.expect_response(
        is_detail_api_response,
        timeout=timeout_seconds * 1000,
    ) as response_info:
        print("CLICK_DETAIL first")
        await click_first_detail(page)
    response = await response_info.value
    meta = await save_detail_api_response(response, output_dir)
    print("CAPTURED_DETAIL_API " + json.dumps(meta, ensure_ascii=False))
    return meta


async def main() -> None:
    parser = argparse.ArgumentParser(description="Capture iDataPlatform API detail response.")
    parser.add_argument("keyword", nargs="?", default=DEFAULT_KEYWORD, help="Report/API search keyword.")
    parser.add_argument("--output", default=".tmp-browser-session-crawler/output", help="Output directory.")
    parser.add_argument("--profile", default=".tmp-browser-session-crawler/profile", help="Browser profile directory.")
    parser.add_argument("--login-timeout", type=int, default=900, help="Seconds to wait for manual login.")
    parser.add_argument("--response-timeout", type=int, default=120, help="Seconds to wait for detail response.")
    parser.add_argument("--detail-api-timeout", type=int, help="Alias for --response-timeout.")
    parser.add_argument("--action-timeout", type=int, default=30, help="Default Playwright action timeout in seconds.")
    parser.add_argument("--keep-open", action="store_true", help="Keep browser open after capture.")
    parser.add_argument(
        "--keep-open-on-error",
        action="store_true",
        help="Keep browser open when capture fails, so login/search/detail state can be inspected.",
    )
    parser.add_argument("--headless", action="store_true", help="Run browser headless.")
    args = parser.parse_args()
    if args.detail_api_timeout is not None:
        args.response_timeout = args.detail_api_timeout

    output_dir = ensure_dir(Path(args.output))
    profile_dir = ensure_dir(Path(args.profile))

    async with async_playwright() as playwright:
        browser_type = playwright.chromium
        channel = "chrome" if os.name == "nt" else None
        context = await browser_type.launch_persistent_context(
            user_data_dir=str(profile_dir.resolve()),
            channel=channel,
            headless=args.headless,
            viewport={"width": 1440, "height": 1000},
            args=["--start-maximized"],
        )
        context.set_default_timeout(args.action_timeout * 1000)
        page = context.pages[0] if context.pages else await context.new_page()
        capture_failed = False

        try:
            await page.goto(SERVICE_SEARCH_URL, wait_until="domcontentloaded", timeout=60000)
            await save_page_artifacts(page, output_dir, "before-search")

            if LOGIN_URL_MARKER in page.url:
                await click_login_method_if_present(page)
                await wait_for_login(page, output_dir, args.login_timeout)
                await page.goto(SERVICE_SEARCH_URL, wait_until="domcontentloaded", timeout=60000)

            await wait_for_login(page, output_dir, args.login_timeout)
            await page.goto(SERVICE_SEARCH_URL, wait_until="domcontentloaded", timeout=60000)
            await fill_search_box(page, args.keyword)
            await click_search_button_if_present(page)
            await wait_for_search_result(page, args.keyword)
            await save_page_artifacts(page, output_dir, "before-detail")

            meta = await capture_detail_response(page, output_dir, args.response_timeout)
            await page.wait_for_timeout(2000)
            await save_page_artifacts(page, output_dir, "detail")
            (output_dir / "state.json").write_text(
                json.dumps(
                    {
                        "keyword": args.keyword,
                        "detailResponse": meta,
                        "pageUrl": page.url,
                    },
                    ensure_ascii=False,
                    indent=2,
                ),
                encoding="utf-8",
            )
            print(json.dumps(meta, ensure_ascii=False, indent=2))
        except Exception as exc:
            capture_failed = True
            await save_error_artifacts(page, output_dir, exc)
            print(
                "CAPTURE_FAILED "
                + json.dumps(
                    {
                        "errorType": type(exc).__name__,
                        "errorMessage": str(exc),
                        "url": page.url,
                        "artifacts": str(output_dir.resolve()),
                    },
                    ensure_ascii=False,
                ),
                file=sys.stderr,
            )
            raise
        finally:
            if args.keep_open or (capture_failed and args.keep_open_on_error and not args.headless):
                print("Browser kept open. Press Ctrl+C in the terminal to close this script.")
                await asyncio.Event().wait()
            await context.close()


if __name__ == "__main__":
    asyncio.run(main())
