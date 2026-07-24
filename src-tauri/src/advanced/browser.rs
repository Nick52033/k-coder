use std::fs;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::cdp::browser_protocol::page::CaptureScreenshotFormat;
use chromiumoxide::page::{Page, ScreenshotParams};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use url::{Host, Url};
use uuid::Uuid;

use crate::protocol::{ToolDefinition, ToolResult};
use crate::storage::now_ms;
use crate::tools::{ToolContext, ToolError, ToolHandler};

use super::store::{append_json_line, read_json_lines};

const MAX_ARTIFACT_BYTES: usize = 8 * 1024 * 1024;
const MAX_SNAPSHOT_CHARS: usize = 120_000;
const MAX_AUDIT_EVENTS: usize = 500;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct BrowserSettings {
    pub enabled: bool,
    pub allow_localhost: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BrowserArtifact {
    pub id: String,
    pub name: String,
    pub media_type: String,
    pub size_bytes: u64,
    pub created_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BrowserAuditEvent {
    pub timestamp_ms: u64,
    pub action: String,
    pub target: String,
    pub success: bool,
    pub detail: String,
}

struct BrowserSession {
    browser: Browser,
    page: Page,
    handler_task: JoinHandle<()>,
}

struct BrowserInner {
    settings_path: PathBuf,
    audit_path: PathBuf,
    artifact_dir: PathBuf,
    settings: RwLock<BrowserSettings>,
    session: Mutex<Option<BrowserSession>>,
}

#[derive(Clone)]
pub struct BrowserService {
    inner: Arc<BrowserInner>,
}

impl BrowserService {
    pub fn new(data_root: &Path) -> Result<Self, String> {
        let root = data_root.join("advanced");
        let settings_path = root.join("browser-settings.json");
        let settings = match fs::read(&settings_path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| error.to_string())?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                BrowserSettings::default()
            }
            Err(error) => return Err(error.to_string()),
        };
        let artifact_dir = root.join("browser-artifacts");
        fs::create_dir_all(&artifact_dir).map_err(|error| error.to_string())?;
        Ok(Self {
            inner: Arc::new(BrowserInner {
                settings_path,
                audit_path: root.join("browser-audit.jsonl"),
                artifact_dir,
                settings: RwLock::new(settings),
                session: Mutex::new(None),
            }),
        })
    }

    pub async fn settings(&self) -> BrowserSettings {
        self.inner.settings.read().await.clone()
    }

    pub async fn save_settings(
        &self,
        settings: BrowserSettings,
    ) -> Result<BrowserSettings, String> {
        if let Some(parent) = self.inner.settings_path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let bytes = serde_json::to_vec_pretty(&settings).map_err(|error| error.to_string())?;
        fs::write(&self.inner.settings_path, bytes).map_err(|error| error.to_string())?;
        *self.inner.settings.write().await = settings.clone();
        if !settings.enabled {
            self.close().await?;
        }
        Ok(settings)
    }

    pub fn audit_events(&self) -> Result<Vec<BrowserAuditEvent>, String> {
        let mut events = read_json_lines(&self.inner.audit_path)?;
        if events.len() > MAX_AUDIT_EVENTS {
            events.drain(..events.len() - MAX_AUDIT_EVENTS);
        }
        Ok(events)
    }

    pub fn artifacts(&self) -> Result<Vec<BrowserArtifact>, String> {
        let mut artifacts = Vec::new();
        for entry in fs::read_dir(&self.inner.artifact_dir).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            let path = entry.path();
            let metadata = entry.metadata().map_err(|error| error.to_string())?;
            if !metadata.is_file()
                || path.extension().and_then(|value| value.to_str()) != Some("png")
            {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            let id = name.strip_suffix(".png").unwrap_or(&name).to_string();
            let created_at_ms = metadata
                .created()
                .or_else(|_| metadata.modified())
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |duration| duration.as_millis() as u64);
            artifacts.push(BrowserArtifact {
                id,
                name,
                media_type: "image/png".into(),
                size_bytes: metadata.len(),
                created_at_ms,
            });
        }
        artifacts.sort_by(|left, right| right.created_at_ms.cmp(&left.created_at_ms));
        Ok(artifacts)
    }

    pub async fn navigate(
        &self,
        url: &str,
        cancellation: CancellationToken,
    ) -> Result<String, String> {
        self.require_enabled().await?;
        let settings = self.settings().await;
        let normalized = validate_url(url, settings.allow_localhost).await?;
        let mut guard = self.inner.session.lock().await;
        self.ensure_session(&mut guard).await?;
        let session = guard.as_mut().expect("browser session initialized");
        let result = tokio::select! {
            _ = cancellation.cancelled() => Err("browser navigation cancelled".to_string()),
            result = session.page.goto(normalized.as_str()) => result.map(|_| normalized.to_string()).map_err(|error| error.to_string()),
        };
        if cancellation.is_cancelled() {
            close_session(&mut guard).await;
        }
        self.audit("navigate", normalized.as_str(), &result);
        result
    }

    pub async fn click(
        &self,
        selector: &str,
        cancellation: CancellationToken,
    ) -> Result<String, String> {
        validate_selector(selector)?;
        self.interact(
            "click",
            selector,
            cancellation,
            |page, selector| async move {
                let element = page
                    .find_element(selector)
                    .await
                    .map_err(|error| error.to_string())?;
                element.click().await.map_err(|error| error.to_string())?;
                Ok("clicked".into())
            },
        )
        .await
    }

    pub async fn type_text(
        &self,
        selector: &str,
        text: &str,
        cancellation: CancellationToken,
    ) -> Result<String, String> {
        validate_selector(selector)?;
        if text.len() > 16_000 {
            return Err("browser input exceeds 16000 characters".into());
        }
        let input = text.to_string();
        self.interact(
            "type",
            selector,
            cancellation,
            move |page, selector| async move {
                let element = page
                    .find_element(selector)
                    .await
                    .map_err(|error| error.to_string())?;
                element.click().await.map_err(|error| error.to_string())?;
                element
                    .type_str(input)
                    .await
                    .map_err(|error| error.to_string())?;
                Ok("typed".into())
            },
        )
        .await
    }

    pub async fn snapshot(&self, cancellation: CancellationToken) -> Result<String, String> {
        self.require_enabled().await?;
        let mut guard = self.inner.session.lock().await;
        self.ensure_session(&mut guard).await?;
        let session = guard.as_mut().expect("browser session initialized");
        let result = tokio::select! {
            _ = cancellation.cancelled() => Err("browser snapshot cancelled".to_string()),
            result = session.page.evaluate_expression("document.body ? document.body.innerText : ''") => {
                match result {
                    Ok(value) => value
                        .into_value::<String>()
                        .map(|value| bound_text(&value, MAX_SNAPSHOT_CHARS))
                        .map_err(|error| error.to_string()),
                    Err(error) => Err(error.to_string()),
                }
            },
        };
        if cancellation.is_cancelled() {
            close_session(&mut guard).await;
        }
        self.audit("snapshot", "active page", &result);
        result
    }

    pub async fn screenshot(
        &self,
        full_page: bool,
        cancellation: CancellationToken,
    ) -> Result<BrowserArtifact, String> {
        self.require_enabled().await?;
        let mut guard = self.inner.session.lock().await;
        self.ensure_session(&mut guard).await?;
        let session = guard.as_mut().expect("browser session initialized");
        let params = ScreenshotParams::builder()
            .format(CaptureScreenshotFormat::Png)
            .full_page(full_page)
            .build();
        let bytes = tokio::select! {
            _ = cancellation.cancelled() => Err("browser screenshot cancelled".to_string()),
            result = session.page.screenshot(params) => result.map_err(|error| error.to_string()),
        };
        if cancellation.is_cancelled() {
            close_session(&mut guard).await;
        }
        let artifact = match bytes {
            Ok(bytes) if bytes.len() <= MAX_ARTIFACT_BYTES => {
                let id = format!("{}-{}", now_ms(), Uuid::new_v4().simple());
                let name = format!("{id}.png");
                fs::write(self.inner.artifact_dir.join(&name), &bytes)
                    .map_err(|error| error.to_string())?;
                Ok(BrowserArtifact {
                    id,
                    name,
                    media_type: "image/png".into(),
                    size_bytes: bytes.len() as u64,
                    created_at_ms: now_ms(),
                })
            }
            Ok(_) => Err("browser screenshot exceeds the 8 MiB artifact limit".into()),
            Err(error) => Err(error),
        };
        self.audit(
            "screenshot",
            if full_page { "full page" } else { "viewport" },
            &artifact,
        );
        artifact
    }

    pub async fn close(&self) -> Result<(), String> {
        let mut guard = self.inner.session.lock().await;
        close_session(&mut guard).await;
        self.audit("close", "active session", &Ok::<_, String>("closed"));
        Ok(())
    }

    async fn interact<F, Fut>(
        &self,
        action: &str,
        target: &str,
        cancellation: CancellationToken,
        operation: F,
    ) -> Result<String, String>
    where
        F: FnOnce(Page, String) -> Fut,
        Fut: std::future::Future<Output = Result<String, String>>,
    {
        self.require_enabled().await?;
        let mut guard = self.inner.session.lock().await;
        self.ensure_session(&mut guard).await?;
        let session = guard.as_ref().expect("browser session initialized");
        let result = tokio::select! {
            _ = cancellation.cancelled() => Err(format!("browser {action} cancelled")),
            result = operation(session.page.clone(), target.to_string()) => result,
        };
        if cancellation.is_cancelled() {
            close_session(&mut guard).await;
        }
        self.audit(action, target, &result);
        result
    }

    async fn require_enabled(&self) -> Result<(), String> {
        if self.inner.settings.read().await.enabled {
            Ok(())
        } else {
            Err("browser automation is disabled; enable it in Settings before use".into())
        }
    }

    async fn ensure_session(&self, guard: &mut Option<BrowserSession>) -> Result<(), String> {
        if guard.is_some() {
            return Ok(());
        }
        let config = BrowserConfig::builder()
            .build()
            .map_err(|error| error.to_string())?;
        let (browser, mut handler) = Browser::launch(config)
            .await
            .map_err(|error| error.to_string())?;
        let handler_task = tokio::spawn(async move {
            while let Some(result) = handler.next().await {
                if result.is_err() {
                    break;
                }
            }
        });
        let page = browser
            .new_page("about:blank")
            .await
            .map_err(|error| error.to_string())?;
        *guard = Some(BrowserSession {
            browser,
            page,
            handler_task,
        });
        Ok(())
    }

    fn audit<T>(&self, action: &str, target: &str, result: &Result<T, String>) {
        let event = BrowserAuditEvent {
            timestamp_ms: now_ms(),
            action: action.into(),
            target: bound_text(target, 500),
            success: result.is_ok(),
            detail: result
                .as_ref()
                .err()
                .map_or_else(|| "ok".into(), |error| bound_text(error, 1000)),
        };
        let _ = append_json_line(&self.inner.audit_path, &event);
    }
}

async fn close_session(guard: &mut Option<BrowserSession>) {
    if let Some(mut session) = guard.take() {
        let _ = session.browser.close().await;
        session.handler_task.abort();
    }
}

async fn validate_url(value: &str, allow_localhost: bool) -> Result<Url, String> {
    let url = Url::parse(value).map_err(|error| format!("invalid browser URL: {error}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("browser navigation only supports HTTP and HTTPS".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("browser URLs must not contain credentials".into());
    }
    let host = url.host().ok_or("browser URL must include a host")?;
    let host_name = host.to_string();
    if host_name.eq_ignore_ascii_case("localhost") {
        return if allow_localhost {
            Ok(url)
        } else {
            Err("localhost browser navigation is disabled".into())
        };
    }
    let addresses = match host {
        Host::Ipv4(address) => vec![IpAddr::V4(address)],
        Host::Ipv6(address) => vec![IpAddr::V6(address)],
        Host::Domain(domain) => {
            tokio::net::lookup_host((domain, url.port_or_known_default().unwrap_or(80)))
                .await
                .map_err(|error| format!("browser host lookup failed: {error}"))?
                .map(|address| address.ip())
                .collect()
        }
    };
    if addresses.is_empty() {
        return Err("browser host did not resolve to an address".into());
    }
    if !allow_localhost && addresses.iter().any(is_non_public_ip) {
        return Err("browser navigation to loopback or private networks is disabled".into());
    }
    Ok(url)
}

fn is_non_public_ip(address: &IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => {
            address.is_private()
                || address.is_loopback()
                || address.is_link_local()
                || address.is_unspecified()
        }
        IpAddr::V6(address) => {
            address.is_loopback()
                || address.is_unspecified()
                || address.is_unique_local()
                || address.is_unicast_link_local()
        }
    }
}

fn validate_selector(selector: &str) -> Result<(), String> {
    if selector.trim().is_empty() || selector.len() > 1000 {
        Err("browser selector must contain 1 to 1000 characters".into())
    } else {
        Ok(())
    }
}

fn bound_text(value: &str, max: usize) -> String {
    if value.len() <= max {
        return value.into();
    }
    let mut end = max;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &value[..end])
}

#[derive(Clone, Copy)]
enum BrowserAction {
    Navigate,
    Click,
    Type,
    Snapshot,
    Screenshot,
    Close,
}

pub struct BrowserTool {
    service: BrowserService,
    action: BrowserAction,
}

impl BrowserTool {
    fn new(service: BrowserService, action: BrowserAction) -> Self {
        Self { service, action }
    }
    pub fn navigate(service: BrowserService) -> Self {
        Self::new(service, BrowserAction::Navigate)
    }
    pub fn click(service: BrowserService) -> Self {
        Self::new(service, BrowserAction::Click)
    }
    pub fn type_text(service: BrowserService) -> Self {
        Self::new(service, BrowserAction::Type)
    }
    pub fn snapshot(service: BrowserService) -> Self {
        Self::new(service, BrowserAction::Snapshot)
    }
    pub fn screenshot(service: BrowserService) -> Self {
        Self::new(service, BrowserAction::Screenshot)
    }
    pub fn close(service: BrowserService) -> Self {
        Self::new(service, BrowserAction::Close)
    }
}

#[async_trait]
impl ToolHandler for BrowserTool {
    fn definition(&self) -> ToolDefinition {
        match self.action {
            BrowserAction::Navigate => ToolDefinition {
                name: "browser_navigate".into(),
                description: "Navigate the approved browser session to a public HTTP(S) URL."
                    .into(),
                input_schema: json!({"type":"object","properties":{"url":{"type":"string","minLength":1,"maxLength":2048}},"required":["url"],"additionalProperties":false}),
            },
            BrowserAction::Click => ToolDefinition {
                name: "browser_click".into(),
                description: "Click an element in the active browser page by CSS selector.".into(),
                input_schema: json!({"type":"object","properties":{"selector":{"type":"string","minLength":1,"maxLength":1000}},"required":["selector"],"additionalProperties":false}),
            },
            BrowserAction::Type => ToolDefinition {
                name: "browser_type".into(),
                description:
                    "Type text into an element in the active browser page by CSS selector.".into(),
                input_schema: json!({"type":"object","properties":{"selector":{"type":"string","minLength":1,"maxLength":1000},"text":{"type":"string","maxLength":16000}},"required":["selector","text"],"additionalProperties":false}),
            },
            BrowserAction::Snapshot => ToolDefinition {
                name: "browser_snapshot".into(),
                description: "Read a bounded plain-text snapshot of the active browser page."
                    .into(),
                input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
            },
            BrowserAction::Screenshot => ToolDefinition {
                name: "browser_screenshot".into(),
                description: "Capture the active browser page into the bounded artifact store."
                    .into(),
                input_schema: json!({"type":"object","properties":{"fullPage":{"type":"boolean"}},"additionalProperties":false}),
            },
            BrowserAction::Close => ToolDefinition {
                name: "browser_close".into(),
                description: "Close the controlled browser session.".into(),
                input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
            },
        }
    }

    async fn execute(
        &self,
        _context: &ToolContext,
        arguments: Value,
        cancellation: CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        let output = match self.action {
            BrowserAction::Navigate => {
                #[derive(Deserialize)]
                struct Args {
                    url: String,
                }
                let args: Args = serde_json::from_value(arguments)
                    .map_err(|error| ToolError::InvalidArguments(error.to_string()))?;
                json!({"url": self.service.navigate(&args.url, cancellation).await.map_err(map_browser_error)?})
            }
            BrowserAction::Click => {
                #[derive(Deserialize)]
                struct Args {
                    selector: String,
                }
                let args: Args = serde_json::from_value(arguments)
                    .map_err(|error| ToolError::InvalidArguments(error.to_string()))?;
                json!({"result": self.service.click(&args.selector, cancellation).await.map_err(map_browser_error)?})
            }
            BrowserAction::Type => {
                #[derive(Deserialize)]
                struct Args {
                    selector: String,
                    text: String,
                }
                let args: Args = serde_json::from_value(arguments)
                    .map_err(|error| ToolError::InvalidArguments(error.to_string()))?;
                json!({"result": self.service.type_text(&args.selector, &args.text, cancellation).await.map_err(map_browser_error)?})
            }
            BrowserAction::Snapshot => {
                json!({"text": self.service.snapshot(cancellation).await.map_err(map_browser_error)?})
            }
            BrowserAction::Screenshot => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct Args {
                    #[serde(default)]
                    full_page: bool,
                }
                let args: Args = serde_json::from_value(arguments)
                    .map_err(|error| ToolError::InvalidArguments(error.to_string()))?;
                serde_json::to_value(
                    self.service
                        .screenshot(args.full_page, cancellation)
                        .await
                        .map_err(map_browser_error)?,
                )
                .map_err(|error| ToolError::Execution(error.to_string()))?
            }
            BrowserAction::Close => {
                self.service.close().await.map_err(ToolError::Execution)?;
                json!({"closed": true})
            }
        };
        Ok(ToolResult {
            success: true,
            output: output.to_string(),
            metadata: json!({"browser": true}),
        })
    }
}

fn map_browser_error(error: String) -> ToolError {
    if error.contains("cancelled") {
        ToolError::Cancelled
    } else {
        ToolError::Execution(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn browser_is_opt_in_and_rejects_private_urls() {
        let directory = tempfile::tempdir().unwrap();
        let service = BrowserService::new(directory.path()).unwrap();
        let error = service
            .snapshot(CancellationToken::new())
            .await
            .unwrap_err();
        assert!(error.contains("disabled"));
        assert!(validate_url("http://127.0.0.1:3000", false).await.is_err());
        assert!(validate_url("file:///tmp/test", false).await.is_err());
        assert!(
            validate_url("http://user:secret@example.com", false)
                .await
                .is_err()
        );
    }
}
