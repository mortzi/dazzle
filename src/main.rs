use dazzle::app::bootstrap::create_app;
use tokio::net::TcpListener;
use tracing::{error, info};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (app, app_state) = create_app().await?;
    let address = format!(
        "{}:{}",
        app_state.config.service_host, app_state.config.service_port
    );
    let listener = TcpListener::bind(&address).await?;

    info!("Server running at {address}");

    tokio::select! {
        result = axum::serve(listener, app) => {
            if let Err(e) = result {
                error!("Server error: {}", e);
            }
        }
        _ = tokio::signal::ctrl_c() => {
            info!("Shutting down");
        }
    }

    Ok(())
}
