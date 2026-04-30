use async_trait::async_trait;

#[async_trait]
pub trait OraclePayloadSource: Send + Sync {
    type PriceId: Send + Sync;
    type Error: std::error::Error + Send + Sync + 'static;

    async fn fetch_payload(&self, price_ids: &[Self::PriceId]) -> Result<Vec<u8>, Self::Error>;
}
