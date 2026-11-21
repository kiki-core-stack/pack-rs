use anyhow::{
    Context,
    Result,
};
use redis::{
    AsyncCommands,
    aio::ConnectionLike,
};
use serde::de::DeserializeOwned;

// Structs

/// Values written by the TypeScript pack's msgpack Redis storage (`storages/redis/msgpack`).
#[derive(Debug)]
pub struct RedisMsgpackStorage;

impl RedisMsgpackStorage {
    pub async fn get_item<T: DeserializeOwned>(
        connection: &mut (impl ConnectionLike + Send + Sync),
        key: &str,
    ) -> Result<Option<T>> {
        let raw_value: Option<Vec<u8>> = connection.get(key).await?;
        raw_value
            .map(|raw_value| rmp_serde::from_slice(&raw_value).context("failed to decode redis msgpack value"))
            .transpose()
    }
}
