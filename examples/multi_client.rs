use dazzle::app::bootstrap::create_app;
use reqwest::Client;
use tokio::net::TcpListener;
use tokio::task::JoinSet;
use tracing::info;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    info!("Starting multi-client");

    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let base_url = format!("http://{}", listener.local_addr()?);

    let (app, _app_state) = create_app().await?;

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    info!("Server running at {base_url}");

    let client = Client::new();
    let mut tasks = JoinSet::new();

    let instruments = ["BTC-PERPETUAL", "ETH-PERPETUAL"]
        .iter()
        .cycle()
        .take(10)
        .copied()
        .collect::<Vec<_>>();

    for instrument in instruments.clone() {
        let client = client.clone();
        let url = format!("{}/ticker/{}", base_url, instrument);
        tasks.spawn(async move {
            match client.get(&url).send().await {
                Ok(res) => {
                    let status = res.status();
                    let body = res.text().await.unwrap_or_default();
                    info!(instrument, %status, "ticker response: {}", &body[..body.len().min(100)]);
                }
                Err(e) => info!(instrument, "request failed: {}", e),
            }
        });
    }

    // SSE stream clients
    for instrument in instruments {
        let client = client.clone();
        let url = format!("{}/ticker/{}/stream", base_url, instrument);
        tasks.spawn(async move {
            info!(instrument, "connecting to stream");
            match client.get(&url).send().await {
                Ok(res) => {
                    use futures::StreamExt;
                    let mut stream = res.bytes_stream();
                    let mut count = 0;
                    while let Some(chunk) = stream.next().await {
                        match chunk {
                            Ok(bytes) => {
                                count += 1;
                                if count % 10 == 0 {
                                    info!(instrument, count, "stream chunks received");
                                }
                            }
                            Err(e) => {
                                info!(instrument, "stream error: {}", e);
                                break;
                            }
                        }
                    }
                }
                Err(e) => info!(instrument, "stream connect failed: {}", e),
            }
        });
    }

    while let Some(result) = tasks.join_next().await {
        if let Err(e) = result {
            info!("task panicked: {}", e);
        }
    }

    Ok(())
}
