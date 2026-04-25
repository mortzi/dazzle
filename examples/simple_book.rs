use dazzle::app::bootstrap::create_app;
use reqwest::Client;
use tokio::net::TcpListener;
use tracing::info;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    info!("Starting multi-client");

    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let base_url = format!("http://{}", listener.local_addr()?);

    let (app, _app_state) = create_app().await?;

    let serve_handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let _cleanup = scopeguard::guard(serve_handle, |h| h.abort());

    info!("Server running at {base_url}");

    let instruments = ["BTC-PERPETUAL", "ETH-PERPETUAL"];

    let client = Client::new();

    for instrument in instruments {
        let client = client.clone();
        let url = format!("{}/book/{}", base_url, instrument);

        match client.get(&url).send().await {
            Ok(res) => {
                let status = res.status();
                let body = res.text().await.unwrap_or_default();
                info!(instrument, %status, "book response: {}", body);
            }
            Err(e) => info!(instrument, "request failed: {}", e),
        }
    }

    Ok(())
}
