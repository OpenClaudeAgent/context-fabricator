use serde::Serialize;

pub const MAX_PAGE: u16 = 1_000;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    pub branches: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_page: Option<u16>,
}
