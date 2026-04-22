use std::env;

#[derive(Debug, Clone)]
pub struct Config {
    pub service_host: String,
    pub service_port: String,
    pub deribit_url: String,
}

impl Config {
    pub fn from_env() -> Result<Self, env::VarError> {
        dotenvy::dotenv().ok();

        Ok(Self {
            service_host: env::var("SERVICE_HOST")?,
            service_port: env::var("SERVICE_PORT")?,
            deribit_url: env::var("DERIBIT_URL")?,
        })
    }
}