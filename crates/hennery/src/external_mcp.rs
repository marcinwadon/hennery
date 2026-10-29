//! A narrow stdio -> HTTP bridge. It cannot read the collector DB or borrow
//! operator cookies. Desktop and other stdio clients use the same scoped API.
use anyhow::{Context, Result, ensure};
use clap::Args;
use std::{
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::PathBuf,
};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};

#[derive(Args)]
pub struct BridgeArgs {
    /// Exact collector MCP URL. HTTPS; HTTP is allowed only on loopback for tests.
    #[arg(long)]
    url: String,
    /// Private (0600), same-user file holding the external client token. Never put tokens in argv.
    #[arg(long)]
    token_file: PathBuf,
}
const MAX_LINE: u64 = 64 * 1024;
const MAX_REPLY: usize = 1024 * 1024;
fn endpoint(value: &str) -> Result<reqwest::Url> {
    let url = reqwest::Url::parse(value).context("invalid external MCP URL")?;
    let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    ensure!(
        url.scheme() == "https" || url.scheme() == "http" && loopback,
        "external MCP requires HTTPS"
    );
    ensure!(
        url.username().is_empty() && url.password().is_none() && url.query().is_none() && url.fragment().is_none(),
        "credentials/query/fragment are not permitted in MCP URL"
    );
    ensure!(
        url.path().ends_with("/mcp/external/hennery"),
        "expected the external MCP endpoint"
    );
    Ok(url)
}
fn token(path: &std::path::Path) -> Result<String> {
    use std::io::Read;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .context("open external client token file")?;
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file()
            && metadata.permissions().mode() & 0o077 == 0
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.len() <= 256,
        "token file must be a private same-user regular file"
    );
    let mut token = String::new();
    file.take(257).read_to_string(&mut token)?;
    ensure!(token.len() <= 256, "external credential exceeds limit");
    let token = token.trim();
    ensure!(
        hennery_sessions::external_clients::valid_token(token),
        "invalid external client credential"
    );
    Ok(token.into())
}
pub async fn run(args: BridgeArgs) -> Result<()> {
    let url = endpoint(&args.url)?;
    let secret = token(&args.token_file)?;
    let mut bearer = reqwest::header::HeaderValue::from_str(&format!("Bearer {secret}"))?;
    bearer.set_sensitive(true);
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(20))
        .build()?;
    let mut input = tokio::io::BufReader::new(tokio::io::stdin());
    let mut output = tokio::io::stdout();
    let mut version: Option<String> = None;
    loop {
        let mut line = vec![];
        let count = (&mut input).take(MAX_LINE + 1).read_until(b'\n', &mut line).await?;
        if count == 0 {
            return Ok(());
        }
        ensure!(count as u64 <= MAX_LINE, "MCP input exceeds 64 KiB");
        let request: serde_json::Value = serde_json::from_slice(&line).context("invalid MCP input JSON")?;
        let mut call = client
            .post(url.clone())
            .header(reqwest::header::AUTHORIZATION, bearer.clone())
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(reqwest::header::ACCEPT, "application/json, text/event-stream")
            .body(line);
        if let Some(version) = &version {
            call = call.header("mcp-protocol-version", version);
        }
        let mut response = call.send().await.context("external MCP connection failed")?;
        if response.status() == reqwest::StatusCode::ACCEPTED {
            continue;
        }
        ensure!(
            response.status().is_success(),
            "external MCP HTTP status {}",
            response.status().as_u16()
        );
        ensure!(
            response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v.starts_with("application/json")),
            "external MCP returned non-JSON response"
        );
        let mut bytes = vec![];
        while let Some(chunk) = response.chunk().await? {
            ensure!(
                bytes.len() + chunk.len() <= MAX_REPLY,
                "external MCP reply exceeds 1 MiB"
            );
            bytes.extend_from_slice(&chunk);
        }
        let value: serde_json::Value = serde_json::from_slice(&bytes).context("invalid external MCP response")?;
        if request.get("method").and_then(serde_json::Value::as_str) == Some("initialize") {
            let negotiated = value
                .pointer("/result/protocolVersion")
                .and_then(serde_json::Value::as_str);
            ensure!(
                negotiated.is_some_and(|version| hennery_sessions::external_mcp::PROTOCOL_VERSIONS.contains(&version)),
                "unsupported MCP negotiation"
            );
            version = negotiated.map(str::to_owned);
        }
        output.write_all(&serde_json::to_vec(&value)?).await?;
        output.write_all(b"\n").await?;
        output.flush().await?;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn collector_url_does_not_allow_credential_downgrades_or_arbitrary_endpoints() {
        assert!(endpoint("https://collector.example/mcp/external/hennery").is_ok());
        assert!(endpoint("http://127.0.0.1:1234/mcp/external/hennery").is_ok());
        for invalid in [
            "http://collector.example/mcp/external/hennery",
            "https://user:pass@collector.example/mcp/external/hennery",
            "https://collector.example/mcp/external/hennery?token=secret",
            "https://collector.example/api/sessions",
        ] {
            assert!(endpoint(invalid).is_err());
        }
    }
    #[test]
    fn token_file_must_be_private_regular_and_not_a_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("token");
        std::fs::write(&path, format!("hnry_external_{}\n", "a".repeat(64))).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(token(&path).is_ok());
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(token(&link).is_err());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(token(&path).is_err());
    }
}
