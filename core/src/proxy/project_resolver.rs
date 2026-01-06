//! Project ID resolver

const CLOUD_CODE_BASE_URL: &str = "https://cloudcode-pa.googleapis.com";

/// Fetch project ID for an account
pub async fn fetch_project_id(access_token: &str) -> anyhow::Result<String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;
    
    let url = format!("{}/v1internal:loadProject", CLOUD_CODE_BASE_URL);
    
    let response = client
        .post(&url)
        .bearer_auth(access_token)
        .json(&serde_json::json!({}))
        .send()
        .await?;
    
    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        anyhow::bail!("loadProject failed with {}: {}", status, text);
    }
    
    let data: serde_json::Value = response.json().await?;
    
    data.get("activeProjectId")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow::anyhow!("No activeProjectId in response"))
}
