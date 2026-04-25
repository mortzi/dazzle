use serde::Deserialize;

#[derive(Debug, Deserialize, Clone)]
pub struct BookQuery {
    pub depth: Option<u32>,
}
